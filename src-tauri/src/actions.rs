//! Side effects of one validated settings mutation.
//!
//! `set_setting` (and the tray menu, the shortcut window and the folder picker that reuse the same
//! path) must do more than persist a value: a watch switch has to actually start/stop a watcher, a
//! clipboard switch has to start/stop the listener, a hotkey has to be re-registered (with
//! rollback), the login item has to be registered/unregistered and a language change has to
//! rebuild the tray. The mapping from "which key changed" to "which side effects run" is a pure
//! function ([`actions_for_change`]) so it can be unit-tested without a Tauri runtime; the
//! application side lives in [`apply`].
//!
//! # Rollback contract
//!
//! Hotkey registration and autostart are the only actions that can *fail visibly*: a conflicting
//! shortcut must keep the old accelerator live (design spec §3.11) and an autostart write can be
//! rejected by the OS. [`apply`] therefore classifies each action as hard (failing it rolls the
//! persisted setting back to its previous value and returns the error) or soft (the setting stays
//! and only a warning is logged). Watcher/clipboard restarts are soft: a temporarily locked folder
//! must not undo the user's switch.

use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use serde_json::Value;
use tauri::{AppHandle, Manager};

use crate::app_state::{self, AppState, PublicState};
use crate::hotkeys;
use crate::panel::{self, GeometryTarget};
use crate::settings::{self, Settings};
use crate::tray;

/// One side effect produced by a settings change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingAction {
    /// Re-register the capture shortcut from the new setting.
    ReRegisterCaptureHotkey,
    /// Re-register the toggle shortcut from the new setting.
    ReRegisterToggleHotkey,
    /// Re-register the "edit latest" shortcut from the new setting (Wave 1 W1.8).
    ReRegisterEditLatestHotkey,
    /// Re-register the "search" shortcut from the new setting (Wave 2 W2.3).
    ReRegisterSearchHotkey,
    /// Stop and restart the folder watchers (watch switches / extra folders changed).
    RestartFolderWatchers,
    /// Start the clipboard listener (`watchClipboard` turned on).
    StartClipboardListener,
    /// Stop the clipboard listener (`watchClipboard` turned off).
    StopClipboardListener,
    /// Register the login item (`startAtLogin` turned on).
    EnableAutostart,
    /// Unregister the login item (`startAtLogin` turned off).
    DisableAutostart,
    /// Rebuild the tray menu in the newly selected language.
    RebuildTray,
    /// Run one age-retention sweep immediately (`keepForDays` changed).
    SweepRetention,
}

impl SettingAction {
    /// Short stable name used in logs and the tray state description.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReRegisterCaptureHotkey => "re-register capture hotkey",
            Self::ReRegisterToggleHotkey => "re-register toggle hotkey",
            Self::ReRegisterEditLatestHotkey => "re-register edit-latest hotkey",
            Self::ReRegisterSearchHotkey => "re-register search hotkey",
            Self::RestartFolderWatchers => "restart folder watchers",
            Self::StartClipboardListener => "start clipboard listener",
            Self::StopClipboardListener => "stop clipboard listener",
            Self::EnableAutostart => "enable autostart",
            Self::DisableAutostart => "disable autostart",
            Self::RebuildTray => "rebuild tray",
            Self::SweepRetention => "sweep age retention",
        }
    }

    /// Whether a failure must roll the persisted setting back.
    ///
    /// Hotkey conflicts and autostart registration are user-visible state that must stay truthful
    /// (design spec §3.11: a rejected shortcut keeps the old one). Watch restarts are best-effort.
    pub fn is_hard(self) -> bool {
        matches!(
            self,
            Self::ReRegisterCaptureHotkey
                | Self::ReRegisterToggleHotkey
                | Self::ReRegisterEditLatestHotkey
                | Self::ReRegisterSearchHotkey
                | Self::EnableAutostart
                | Self::DisableAutostart
        )
    }
}

/// Returns the side effects the change `old -> new` for `key` must run.
///
/// Only the changed key's own effects are returned; an unrelated key (`theme`, `sound`, …) needs no
/// Rust-side action because the frontend follows the broadcast `state` event. The function is
/// defensive: if a caller passes a key whose value did not actually change (a repeated menu click),
/// the corresponding action is not scheduled, so the watchers are not pointlessly restarted.
/// `startAtLogin` is the deliberate exception: it is an idempotent reconcile of the real registry
/// state (the tray toggle target comes from that state, not from the persisted bool).
pub fn actions_for_change(key: &str, old: &Settings, new: &Settings) -> Vec<SettingAction> {
    let mut actions = Vec::new();
    match key {
        "captureHotkey" if old.capture_hotkey != new.capture_hotkey => {
            actions.push(SettingAction::ReRegisterCaptureHotkey);
        }
        "toggleHotkey" if old.toggle_hotkey != new.toggle_hotkey => {
            actions.push(SettingAction::ReRegisterToggleHotkey);
        }
        "editLatestHotkey" if old.edit_latest_hotkey != new.edit_latest_hotkey => {
            actions.push(SettingAction::ReRegisterEditLatestHotkey);
        }
        "searchHotkey" if old.search_hotkey != new.search_hotkey => {
            actions.push(SettingAction::ReRegisterSearchHotkey);
        }
        "watchSystemScreenshots"
            if old.watch_system_screenshots != new.watch_system_screenshots =>
        {
            actions.push(SettingAction::RestartFolderWatchers);
        }
        "watchRecordings" if old.watch_recordings != new.watch_recordings => {
            actions.push(SettingAction::RestartFolderWatchers);
        }
        "extraFolders" if old.extra_folders != new.extra_folders => {
            actions.push(SettingAction::RestartFolderWatchers);
        }
        "watchClipboard" if old.watch_clipboard != new.watch_clipboard => {
            actions.push(if new.watch_clipboard {
                SettingAction::StartClipboardListener
            } else {
                SettingAction::StopClipboardListener
            });
        }
        // The login item is deliberately re-asserted even when the persisted value did not change:
        // the tray derives the click target from the *real* registration, so with a manual `Run`
        // entry (setting already false) the first click must remove it instead of writing `true`.
        // Treating "set startAtLogin to its current value" as an explicit reconcile keeps the tray,
        // the settings UI and `set_setting` on one semantic.
        "startAtLogin" => {
            actions.push(if new.start_at_login {
                SettingAction::EnableAutostart
            } else {
                SettingAction::DisableAutostart
            });
        }
        "language" if old.language != new.language => {
            actions.push(SettingAction::RebuildTray);
        }
        // v1.4 appearance: the tray is the only surface for the rope style/colour, so the change
        // must refresh the native check marks immediately. `apply_menu_action` returns through this
        // path and `tray::sync_state` would already rewrite the checks during the state broadcast;
        // the explicit RebuildTray keeps the "immediate" contract testable and is idempotent.
        "ropeStyle" if old.rope_style != new.rope_style => {
            actions.push(SettingAction::RebuildTray);
        }
        "ropeColor" if old.rope_color != new.rope_color => {
            actions.push(SettingAction::RebuildTray);
        }
        // A shorter (or newly enabled) age window must take effect now, not at the next hourly tick.
        // The sweep itself is soft: it reports failures through logs and never rolls the setting back.
        "keepForDays" if old.keep_for_days != new.keep_for_days => {
            actions.push(SettingAction::SweepRetention);
        }
        _ => {}
    }
    actions
}

/// Maps a hotkey setting key to its [`hotkeys::Target`].
///
/// The shortcut window and `set_hotkey` both need this mapping; it is kept next to
/// [`actions_for_change`] so the two hotkey keys cannot drift apart.
pub fn hotkey_target_for_key(key: &str) -> Option<hotkeys::Target> {
    match key {
        "captureHotkey" => Some(hotkeys::Target::Capture),
        "toggleHotkey" => Some(hotkeys::Target::Toggle),
        "editLatestHotkey" => Some(hotkeys::Target::EditLatest),
        "searchHotkey" => Some(hotkeys::Target::Search),
        _ => None,
    }
}

/// Applies one `set_setting` write together with its side effects.
///
/// Order of operations:
/// 1. validate + persist the new value (`AppState::set_json`; a rejected value never reaches here);
/// 2. run the planned side effects — hard failures restore the previous settings and return the
///    error, soft failures only log (a `keepForDays` change also starts the immediate age sweep,
///    which runs on its own thread and reports any problem through the logs);
/// 3. re-apply the panel geometry for the size/limit keys (single geometry owner, Task 14);
/// 4. broadcast the new full state and refresh the tray.
///
/// The returned snapshot is the one that was broadcast, so the IPC reply never regresses the
/// frontend store.
pub fn apply(app: &AppHandle, key: &str, value: Value) -> Result<PublicState, String> {
    let state = app.state::<AppState>();
    let before = state.snapshot().settings;
    let applied = state.set_json(key, value).map_err(|err| format!("{err:#}"))?;
    let after = applied.settings.clone();
    let actions = actions_for_change(key, &before, &after);

    if let Err(error) = run_actions(app, &after, &actions) {
        // Hard failure: the previous settings/registration are still the truth; restore the file
        // and the in-memory state so the tray/frontend do not show a value that is not in effect.
        log::error!(
            "settings: {key} rolled back after a failed {}: {error}",
            actions
                .iter()
                .find(|action| action.is_hard())
                .map(|action| action.as_str())
                .unwrap_or("side effect")
        );
        if let Err(restore_error) = state.restore_settings(before) {
            log::error!("settings: failed to persist the {key} rollback: {restore_error:#}");
        }
        let _ = app_state::emit_state(app);
        tray::sync_state(app);
        return Err(error);
    }

    if settings::affects_panel_geometry(key) {
        let geometry = panel::sync_panel_geometry(app, GeometryTarget::CurrentPanel);
        log::info!(
            "ipc: set_setting {key} persisted; panel geometry re-applied (moved={}, target {:?})",
            geometry.moved,
            geometry
                .target
                .as_ref()
                .map(|monitor| monitor.id.as_str())
        );
    } else {
        log::info!("ipc: set_setting {key} persisted");
    }

    let snapshot = app_state::emit_state(app).unwrap_or(applied);
    tray::sync_state(app);
    Ok(snapshot)
}

/// Runs the planned actions, returning the first hard failure.
///
/// Soft actions are attempted even when an earlier soft action failed; a hard failure stops the
/// sequence (the caller rolls the setting back, so later hard actions would act on a value that is
/// about to be reverted).
fn run_actions(
    app: &AppHandle,
    settings: &Settings,
    actions: &[SettingAction],
) -> Result<(), String> {
    for action in actions {
        log::info!("settings: applying {}", action.as_str());
        let result = match action {
            SettingAction::ReRegisterCaptureHotkey => {
                hotkeys::register(app, hotkeys::Target::Capture, &settings.capture_hotkey)
                    .map_err(|err| format!("capture shortcut {}: {err}", settings.capture_hotkey))
            }
            SettingAction::ReRegisterToggleHotkey => {
                hotkeys::register(app, hotkeys::Target::Toggle, &settings.toggle_hotkey)
                    .map_err(|err| format!("show/hide shortcut {}: {err}", settings.toggle_hotkey))
            }
            SettingAction::ReRegisterEditLatestHotkey => hotkeys::register(
                app,
                hotkeys::Target::EditLatest,
                &settings.edit_latest_hotkey,
            )
            .map_err(|err| {
                format!(
                    "edit-latest shortcut {}: {err}",
                    settings.edit_latest_hotkey
                )
            }),
            SettingAction::ReRegisterSearchHotkey => {
                hotkeys::register(app, hotkeys::Target::Search, &settings.search_hotkey)
                    .map_err(|err| format!("search shortcut {}: {err}", settings.search_hotkey))
            }
            SettingAction::RestartFolderWatchers => {
                crate::watchers::restart_folder_watchers(app);
                Ok(())
            }
            SettingAction::StartClipboardListener => {
                crate::watchers::start_clipboard(app);
                Ok(())
            }
            SettingAction::StopClipboardListener => {
                crate::watchers::stop_clipboard(app);
                Ok(())
            }
            SettingAction::EnableAutostart => match crate::autostart::enable(app) {
                Ok(()) => {
                    log::info!("settings: registered Hangline as a login item");
                    Ok(())
                }
                Err(err) => Err(format!("cannot enable start at login: {err}")),
            },
            SettingAction::DisableAutostart => match crate::autostart::disable(app) {
                Ok(()) => {
                    log::info!("settings: removed Hangline from the login items");
                    Ok(())
                }
                Err(err) => Err(format!("cannot disable start at login: {err}")),
            },
            SettingAction::RebuildTray => {
                // The language changed; every label has to be re-rendered. A failure here is
                // cosmetic (the next `sync_state` retries), never a rollback reason.
                tray::rebuild(app);
                Ok(())
            }
            SettingAction::SweepRetention => {
                // Runs after the value was persisted; the worker trims the state, broadcasts the
                // trimmed snapshot and recycles the expired owned screenshots on its own thread.
                crate::retention::sweep_async(app);
                Ok(())
            }
        };
        match result {
            Ok(()) => {}
            Err(error) if action.is_hard() => return Err(error),
            Err(error) => log::warn!("settings: soft action {} failed: {error}", action.as_str()),
        }
    }
    Ok(())
}

/// Documented placeholder Releases page, kept **only as the fallback link** for updater errors.
///
/// P5-2 replaced the old "open the Releases page" behavior with a real manual check through
/// `tauri-plugin-updater` (the update endpoint is the app's only network destination). This URL is
/// no longer opened automatically: it is logged as a manual fallback when a check cannot reach the
/// endpoint, until the user publishes the real repository Releases page (plan reference data).
pub const RELEASES_URL: &str = "https://github.com/devin2255/hangline/releases";

/// Takes every card off the line, reusing the Task 12 take-down rule per item.
///
/// Files Hangline owns (inside [`crate::capture::screenshots_dir`]) are recycled into the Recycle
/// Bin; watched-folder/system items only leave the line (design spec §3.1, "inbox mode"). A file
/// that cannot be recycled keeps its card (the recycle decision may not be silently dropped). The
/// caller is a background thread (tray menu) because `IFileOperation` can block on the shell.
///
/// Returns the number of cards removed.
pub fn take_everything_down(app: &AppHandle) -> usize {
    let Some(state) = app.try_state::<AppState>() else {
        log::warn!("take_everything_down: app state is not managed");
        return 0;
    };
    let items = state.snapshot().items;
    if items.is_empty() {
        log::info!("take_everything_down: the line is already empty");
        return 0;
    }
    let own_dir = match crate::capture::screenshots_dir() {
        Ok(dir) => Some(dir),
        Err(error) => {
            log::warn!(
                "take_everything_down: cannot resolve the screenshot directory ({error:#}); \
                 only taking cards off the line, no file will be recycled"
            );
            None
        }
    };

    let mut removed = 0usize;
    for item in items {
        if let Some(own_dir) = own_dir.as_deref() {
            let requested = crate::trash::take_down_action(item.kind, &item.path, own_dir);
            let action = crate::trash::resolve_action(requested, &item.path);
            if action == crate::trash::TakeDownAction::Recycle {
                if let Err(error) = crate::trash::recycle(&item.path) {
                    log::warn!(
                        "take_everything_down: cannot recycle {} ({error:#}); keeping its card",
                        item.path.display()
                    );
                    continue;
                }
            }
        }
        match state.remove(&item.id) {
            Ok(Some(_)) => removed += 1,
            Ok(None) => log::info!(
                "take_everything_down: {} left the line meanwhile",
                item.path.display()
            ),
            Err(error) => {
                log::error!(
                    "take_everything_down: cannot persist the removal of {}: {error:#}",
                    item.path.display()
                );
                break;
            }
        }
    }

    let _ = app_state::emit_state(app);
    crate::sound::discarded(app);
    log::info!("take_everything_down: {removed} card(s) taken off the line");
    removed
}

/// The card the `editLatest` hotkey opens: the newest row of the line.
///
/// `PublicState::items` is ordered by `added_at` ascending (the newest hangs on the right), so the
/// newest card is the last row. Pure, so the W1.8 selection is unit-tested without a state.
pub fn newest_item(items: &[crate::item::Item]) -> Option<&crate::item::Item> {
    items.last()
}

/// Opens the editor for the newest card on the line (W1.8 `editLatest` hotkey).
///
/// Runs on a named worker thread: window creation dispatches to the event loop and waits (the
/// global-shortcut handler thread must not block) and a recording's duration probe blocks on the
/// media stack. An empty line is only logged — the spec asks for no dialog here.
pub fn edit_latest_in_background(app: &AppHandle) {
    let app = app.clone();
    let spawned = std::thread::Builder::new()
        .name("hangline-edit-latest".to_string())
        .spawn(move || {
            if let Err(error) = edit_latest(&app) {
                log::warn!("edit latest: cannot open the editor: {error:#}");
            }
        });
    if let Err(error) = spawned {
        log::error!("edit latest: cannot spawn the editor worker thread: {error}");
    }
}

/// Body of [`edit_latest_in_background`]: recordings open the trim window, every image kind opens
/// the markup editor.
fn edit_latest(app: &AppHandle) -> anyhow::Result<()> {
    let Some(state) = app.try_state::<AppState>() else {
        return Err(anyhow::anyhow!("edit latest: the app state is not managed"));
    };
    let items = state.snapshot().items;
    let Some(item) = newest_item(&items).cloned() else {
        log::info!("edit latest: the line is empty; nothing to open");
        return Ok(());
    };
    log::info!(
        "edit latest: opening {} ({:?})",
        item.path.display(),
        item.kind
    );
    if item.kind == crate::item::ItemKind::Recording {
        // Resolving the target blocks on the WinRT duration probe; this worker thread may block.
        let target = crate::trim::target(app, &item.id)?;
        let lang = state.snapshot().resolved_language;
        crate::trim::open(app, target, &lang)
    } else {
        crate::annotate::open(app, &item.id)
    }
}

/// Serializes the `extraFolders` add/remove mutations (Task 15 review fix).
///
/// A `SHBrowseForFolderW` dialog is only modal to its own thread, so two "Add Folder…" pickers can
/// be open at once and the tray menu can remove a folder while a picker is open. Every tray-side
/// folder mutation therefore takes this lock for its short read-current → merge → `set_setting`
/// section: at most one of them reads and writes the list at a time, and the base is always the
/// *current* list (never the snapshot taken before the dialog opened).
static FOLDER_MUTATION: Mutex<()> = Mutex::new(());

/// Locks the folder-mutation critical section; a poisoned lock is recovered (the settings are the
/// authoritative state, the guard only serializes the read-modify-write).
fn folder_mutation_guard() -> MutexGuard<'static, ()> {
    FOLDER_MUTATION
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The current `extraFolders` list (re-read *after* the modal picker closed).
fn current_extra_folders(app: &AppHandle) -> Option<Vec<String>> {
    app.try_state::<AppState>()
        .map(|state| state.snapshot().settings.extra_folders)
}

/// Merges a freshly picked directory into the **current** `extraFolders` list.
///
/// This is the pure decision behind "Add Folder…" after the modal picker returns: the caller
/// re-reads the current settings and passes them as `current`, so a folder added or removed while
/// the dialog was open is never dropped or resurrected. The chosen path must be an existing
/// directory; absolute-path, duplicate (Windows case-insensitive / trailing-separator insensitive)
/// and the three-folder cap checks are the same rules `set_json` applies as the final gate.
///
/// Errors are human-readable log lines for the tray.
pub fn merge_picked_folder(current: &[String], picked: &Path) -> Result<Vec<String>, String> {
    if !picked.is_dir() {
        return Err(format!(
            "{} is not an existing directory",
            picked.display()
        ));
    }
    settings::folder_list_with(current, picked)
}

/// Opens the watched folder at `index` in Explorer (the current list is the source of truth).
pub fn open_watched_folder(app: &AppHandle, index: usize) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let snapshot = state.snapshot();
    let Some(folder) = snapshot.settings.extra_folders.get(index) else {
        log::warn!("tray: watched-folder row {index} is stale (the list changed)");
        return;
    };
    if let Err(error) = crate::trash::open_path(std::path::Path::new(folder)) {
        log::warn!("tray: cannot open the watched folder {folder}: {error:#}");
    }
}

/// Shows the system folder picker and adds the chosen folder to `extraFolders` (≤3).
///
/// Runs on a background thread: `SHBrowseForFolderW` is modal until the user picks/cancels.
/// Cancel is a logged no-op. The pre-check below only avoids opening a dialog that cannot succeed
/// (list already at the cap at click time); the authoritative cap/duplicate/existence check happens
/// **after** the dialog closes, under [`FOLDER_MUTATION`], against the current list.
pub fn add_watched_folder(app: &AppHandle) {
    // Cheap pre-check: do not open a dialog that is already known to be pointless. The list may
    // change while the dialog is open; `add_picked_folder` re-checks against the current list.
    if let Some(folders) = current_extra_folders(app) {
        if folders.len() >= crate::settings::MAX_EXTRA_FOLDERS {
            log::warn!(
                "tray: cannot add another watched folder ({} of {} in use)",
                folders.len(),
                crate::settings::MAX_EXTRA_FOLDERS
            );
            return;
        }
    }
    let strings = crate::i18n::tray_strings(crate::i18n::app_language(app));
    let Some(folder) = crate::folder_picker::pick_folder(None, strings.picker_title) else {
        log::info!("tray: add-folder cancelled");
        return;
    };
    add_picked_folder(app, &folder);
}

/// Applies one already-picked folder against the current settings (serialized folder mutation).
fn add_picked_folder(app: &AppHandle, folder: &Path) {
    let _guard = folder_mutation_guard();
    let Some(current) = current_extra_folders(app) else {
        return;
    };
    let folders = match merge_picked_folder(&current, folder) {
        Ok(folders) => folders,
        Err(error) => {
            log::warn!("tray: cannot watch {}: {error}", folder.display());
            return;
        }
    };
    match apply(app, "extraFolders", Value::from(folders)) {
        Ok(snapshot) => log::info!(
            "tray: watching {} extra folder(s) now (added {}; merged into the current list)",
            snapshot.settings.extra_folders.len(),
            folder.display()
        ),
        Err(error) => log::warn!("tray: cannot persist the extra folders: {error}"),
    }
}

/// Removes the watched folder at `index` from `extraFolders` (asset scope is not revoked).
///
/// Serialized with [`add_watched_folder`] via [`FOLDER_MUTATION`] and reads the current list under
/// the lock, so a folder removed while another picker is open is not resurrected by a stale index.
pub fn stop_watching_folder(app: &AppHandle, index: usize) {
    let _guard = folder_mutation_guard();
    let Some(current) = current_extra_folders(app) else {
        return;
    };
    let Some(folder) = current.get(index).cloned() else {
        log::warn!("tray: stop-watching row {index} is stale (the list changed)");
        return;
    };
    let folders = crate::settings::folder_list_without(&current, index);
    match apply(app, "extraFolders", Value::from(folders)) {
        Ok(_) => log::info!(
            "tray: stopped watching {folder} (the asset-protocol scope is not revoked; \
             cards already on the line keep their thumbnails)"
        ),
        Err(error) => log::warn!("tray: cannot remove the watched folder {folder}: {error}"),
    }
}

/// Opens Hangline's own screenshot directory in Explorer.
pub fn open_screenshots_folder(_app: &AppHandle) {
    let dir = match crate::capture::screenshots_dir() {
        Ok(dir) => dir,
        Err(error) => {
            log::warn!("tray: cannot resolve the screenshot directory: {error:#}");
            return;
        }
    };
    if let Err(error) = crate::trash::open_path(&dir) {
        log::warn!("tray: cannot open the screenshot directory: {error:#}");
    }
}

/// "Check for updates…" (manual): runs a real update check and announces/asks the panel.
///
/// The tray menu handler runs this on a background thread; the check itself is async and is moved
/// to the Tauri runtime. `updater::check(_, true)` always bypasses the 24 h window, emits the
/// manual-check events, caches an announcement and shows the panel when there is a result (the
/// notice carries the `Install update` button). A failure never opens a browser: the log keeps
/// [`RELEASES_URL`] as a manual fallback link and the panel shows "Couldn't check for updates".
pub fn check_for_updates(app: &AppHandle) {
    log::info!(
        "tray: manual update check requested (the updater endpoint is the only network access)"
    );
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        match crate::updater::check(&handle, true).await {
            Ok(Some(info)) => {
                log::info!(
                    "tray: update {} is available (manual check); the panel offers the install action",
                    info.version
                );
                show_line_for_update_result(&handle);
            }
            Ok(None) => log::info!("tray: manual update check found no newer release"),
            Err(error) => {
                log::warn!(
                    "tray: manual update check failed: {error}; manual fallback Releases page: \
                     {RELEASES_URL}"
                );
                show_line_for_update_result(&handle);
            }
        }
    });
}

/// Reveals the line when a manual update result would otherwise be invisible.
///
/// The notice lives in the panel; a user who clicks the tray item while the line is tucked must
/// see the availability/install button (or the failure notice). A visible line is left untouched.
fn show_line_for_update_result(app: &AppHandle) {
    if !crate::panel::is_visible() {
        log::info!("tray: revealing the line for the manual update result");
        crate::reveal::toggle(app, crate::reveal::RevealReason::Toggle);
    }
}

/// Graceful Quit: stop the folder watchers and the clipboard listener, then exit.
///
/// The spec's "还原状态并退出" is a no-op on Windows (Hangline never changes a system setting), but
/// the watcher threads are stopped before the process ends so no listener survives the quit. The
/// `RunEvent::Exit` hook in `lib.rs` calls the same idempotent shutdown for exits that do not go
/// through this function (logoff, process kill requested through Tauri).
pub fn quit(app: &AppHandle) {
    log::info!("tray: quit requested; stopping the watchers and clipboard listener");
    crate::watchers::shutdown(app);
    app.exit(0);
}


#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;
    use std::path::PathBuf;

    use serde_json::json;

    use crate::settings::{CardSize, KeepForDays, KeepOnLine};

    /// Scratch directory under `target/` (git-ignored); never touches `%LOCALAPPDATA%`.
    fn test_dir(tag: &str) -> PathBuf {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("actions-tests")
            .join(format!("{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create the actions test dir");
        dir
    }

    /// Creates a real subdirectory and returns its string form (the watcher requires `is_dir`).
    fn watched_dir(base: &Path, name: &str) -> String {
        let dir = base.join(name);
        fs::create_dir_all(&dir).expect("create a test watched folder");
        dir.to_string_lossy().into_owned()
    }

    /// Task 15 review fix: the merge must use the *current* list captured after the picker closes.
    /// A stale snapshot (taken before the dialog opened) can contain a folder that was removed in
    /// the meantime; using it would resurrect that folder or drop a concurrent addition.
    #[test]
    fn merge_picked_folder_uses_the_current_list_not_a_stale_snapshot() {
        let base = test_dir("folder-merge");
        let a = watched_dir(&base, "a");
        let b = watched_dir(&base, "b");
        let c = watched_dir(&base, "c");

        // The picker opened when the list was [A, B]; while it was open B was removed again, so the
        // current list is [A]. The merge must append C to [A] and never reintroduce B.
        let stale_snapshot = vec![a.clone(), b.clone()];
        let current = vec![a.clone()];
        assert!(stale_snapshot.contains(&b), "the stale base still has B");
        let merged = merge_picked_folder(&current, Path::new(&c)).expect("merge the picked folder");
        assert_eq!(merged, vec![a.clone(), c.clone()]);
        assert!(
            !merged.contains(&b),
            "a folder removed while the dialog was open must not be resurrected"
        );

        // A folder added while the dialog was open is in `current` and must be kept.
        let current_with_addition = vec![a.clone(), b.clone()];
        let merged = merge_picked_folder(&current_with_addition, Path::new(&c))
            .expect("merge onto the current list");
        assert_eq!(merged, vec![a.clone(), b.clone(), c.clone()]);

        let _ = fs::remove_dir_all(base);
    }

    /// Existence, duplicate (Windows case/trailing-separator insensitive) and the three-folder cap
    /// are all re-checked against the current list after the modal dialog.
    #[test]
    fn merge_picked_folder_validates_existence_duplicates_and_cap() {
        let base = test_dir("folder-merge-rules");
        let a = watched_dir(&base, "a");
        let b = watched_dir(&base, "b");
        let c = watched_dir(&base, "c");
        let missing = base.join("does-not-exist");
        let current = vec![a.clone()];

        // A chosen path that is not a directory (deleted while the dialog was open, or a file) is
        // rejected instead of being persisted.
        assert!(merge_picked_folder(&current, &missing).is_err());
        // Duplicate of the current list (case + trailing separator folding).
        assert!(merge_picked_folder(&current, Path::new(&a)).is_err());
        let noisy = format!("{}\\", a.to_uppercase());
        assert!(
            merge_picked_folder(&current, Path::new(&noisy)).is_err(),
            "case/trailing-separator duplicate must be rejected"
        );

        // The cap is applied to the current list, not to a possibly stale one.
        let at_cap = vec![a.clone(), b.clone(), c.clone()];
        let extra = watched_dir(&base, "d");
        assert!(merge_picked_folder(&at_cap, Path::new(&extra)).is_err());
        // Two folders still allow one more.
        let two = vec![a, b];
        assert_eq!(
            merge_picked_folder(&two, Path::new(&extra))
                .expect("third folder is allowed")
                .len(),
            3
        );

        let _ = fs::remove_dir_all(base);
    }

    /// A settings value changed in place, mirroring what `set_json` would have produced.
    fn changed(key: &str, value: Value) -> (Settings, Settings) {
        let old = Settings::default();
        let mut new = old.clone();
        new.set_json(key, value)
            .unwrap_or_else(|err| panic!("test change {key} must be valid: {err}"));
        (old, new)
    }

    fn actions(key: &str, value: Value) -> Vec<SettingAction> {
        let (old, new) = changed(key, value);
        actions_for_change(key, &old, &new)
    }

    /// Each spec 3.6 key maps onto the side effects it actually needs; the watch switches are the
    /// ones Task 15 must turn into real watcher/clipboard restarts.
    #[test]
    fn setting_keys_map_to_their_side_effects() {
        assert_eq!(
            actions("watchSystemScreenshots", json!(false)),
            vec![SettingAction::RestartFolderWatchers]
        );
        assert_eq!(
            actions("watchRecordings", json!(false)),
            vec![SettingAction::RestartFolderWatchers]
        );
        assert_eq!(
            actions("extraFolders", json!(["C:\\Shots"])),
            vec![SettingAction::RestartFolderWatchers]
        );
        assert_eq!(
            actions("watchClipboard", json!(false)),
            vec![SettingAction::StopClipboardListener]
        );
        assert_eq!(
            actions("watchClipboard", json!(true)),
            vec![],
            "turning it on when it is already on is not a change"
        );
        // Turn it off, then back on: the start action is produced.
        let mut old = Settings::default();
        old.watch_clipboard = false;
        let mut new = old.clone();
        new.watch_clipboard = true;
        assert_eq!(
            actions_for_change("watchClipboard", &old, &new),
            vec![SettingAction::StartClipboardListener]
        );
        assert_eq!(
            actions("captureHotkey", json!("Ctrl+Alt+Shift+F5")),
            vec![SettingAction::ReRegisterCaptureHotkey]
        );
        assert_eq!(
            actions("toggleHotkey", json!("Ctrl+Alt+Shift+T")),
            vec![SettingAction::ReRegisterToggleHotkey]
        );
        assert_eq!(
            actions("editLatestHotkey", json!("Ctrl+Shift+E")),
            vec![SettingAction::ReRegisterEditLatestHotkey]
        );
        assert_eq!(
            actions("searchHotkey", json!("Ctrl+Shift+F")),
            vec![SettingAction::ReRegisterSearchHotkey]
        );
        assert_eq!(
            actions("startAtLogin", json!(true)),
            vec![SettingAction::EnableAutostart]
        );
        let mut old = Settings::default();
        old.start_at_login = true;
        let new = Settings::default();
        assert_eq!(
            actions_for_change("startAtLogin", &old, &new),
            vec![SettingAction::DisableAutostart]
        );
        assert_eq!(
            actions("language", json!("zh-Hans")),
            vec![SettingAction::RebuildTray]
        );
        // Age retention: changing the window must run the sweep immediately (soft action).
        assert_eq!(
            actions("keepForDays", json!(7)),
            vec![SettingAction::SweepRetention]
        );
        // Turning the policy off again is a change too: sweep once with the new (forever) policy.
        let mut old = Settings::default();
        old.keep_for_days = KeepForDays::Days(30);
        let mut new = old.clone();
        new.keep_for_days = KeepForDays::Forever;
        assert_eq!(
            actions_for_change("keepForDays", &old, &new),
            vec![SettingAction::SweepRetention]
        );
    }

    /// Palette/theme/size-like keys are frontend-only or handled by the geometry path: they must
    /// not start watchers or touch the login item.
    #[test]
    fn frontend_only_settings_need_no_rust_action() {
        for (key, value) in [
            ("theme", json!("dark")),
            ("cardSize", json!("large")),
            ("keepOnLine", json!(25)),
            ("sound", json!(false)),
            ("revealOnNew", json!(false)),
            ("revealPeekMs", json!(1000)),
            ("topEdgeReveal", json!(true)),
            ("showOverFullScreen", json!(true)),
            ("autoCheckUpdates", json!(false)),
            ("lastUpdateCheck", json!("2026-10-29T13:00:00Z")),
        ] {
            let (old, new) = changed(key, value);
            assert!(
                actions_for_change(key, &old, &new).is_empty(),
                "{key} must not schedule a Rust side effect"
            );
        }
    }

    /// A menu click that re-selects the current value must not restart anything. `startAtLogin` is
    /// excluded on purpose: it re-asserts the real registration even when the bool is unchanged.
    #[test]
    fn unchanged_values_schedule_nothing() {
        let old = Settings::default();
        let new = Settings::default();
        for key in [
            "captureHotkey",
            "toggleHotkey",
            "editLatestHotkey",
            "searchHotkey",
            "watchSystemScreenshots",
            "watchRecordings",
            "watchClipboard",
            "extraFolders",
            "language",
            "keepForDays",
            "ropeStyle",
            "ropeColor",
        ] {
            assert!(
                actions_for_change(key, &old, &new).is_empty(),
                "{key} with the same value must not schedule a side effect"
            );
        }
    }

    /// v1.4: changing the rope style/colour must refresh the tray immediately (the appearance
    /// submenu's check marks are the only UI for these keys).
    #[test]
    fn appearance_settings_rebuild_the_tray() {
        for (key, value) in [
            ("ropeStyle", json!("aurora")),
            ("ropeColor", json!("terracotta")),
        ] {
            let (old, new) = changed(key, value);
            assert_eq!(
                actions_for_change(key, &old, &new),
                vec![SettingAction::RebuildTray],
                "{key} must refresh the tray check marks immediately"
            );
        }
        // Re-selecting the current value is a no-op (no pointless rebuild).
        let old = Settings::default();
        let new = Settings::default();
        for key in ["ropeStyle", "ropeColor"] {
            assert!(actions_for_change(key, &old, &new).is_empty());
        }
    }

    /// The tray toggle derives its target from the real registration, so an unchanged
    /// `startAtLogin` must still run the matching enable/disable action: with a manual `Run` entry
    /// and `startAtLogin=false`, the first click has to remove that entry (not write `true`).
    #[test]
    fn start_at_login_is_reasserted_even_when_unchanged() {
        let off = Settings::default();
        assert_eq!(
            actions_for_change("startAtLogin", &off, &off),
            vec![SettingAction::DisableAutostart]
        );
        let mut on = Settings::default();
        on.start_at_login = true;
        assert_eq!(
            actions_for_change("startAtLogin", &on, &on),
            vec![SettingAction::EnableAutostart]
        );
    }

    #[test]
    fn hard_actions_are_the_ones_that_roll_back() {
        for action in [
            SettingAction::ReRegisterCaptureHotkey,
            SettingAction::ReRegisterToggleHotkey,
            SettingAction::ReRegisterEditLatestHotkey,
            SettingAction::ReRegisterSearchHotkey,
            SettingAction::EnableAutostart,
            SettingAction::DisableAutostart,
        ] {
            assert!(action.is_hard(), "{} must roll back on failure", action.as_str());
        }
        for action in [
            SettingAction::RestartFolderWatchers,
            SettingAction::StartClipboardListener,
            SettingAction::StopClipboardListener,
            SettingAction::RebuildTray,
            SettingAction::SweepRetention,
        ] {
            assert!(
                !action.is_hard(),
                "{} must be best-effort, not a rollback reason",
                action.as_str()
            );
        }
    }

    #[test]
    fn hotkey_keys_map_to_targets() {
        assert_eq!(
            hotkey_target_for_key("captureHotkey"),
            Some(hotkeys::Target::Capture)
        );
        assert_eq!(
            hotkey_target_for_key("toggleHotkey"),
            Some(hotkeys::Target::Toggle)
        );
        assert_eq!(
            hotkey_target_for_key("editLatestHotkey"),
            Some(hotkeys::Target::EditLatest)
        );
        assert_eq!(
            hotkey_target_for_key("searchHotkey"),
            Some(hotkeys::Target::Search)
        );
        assert_eq!(hotkey_target_for_key("cardSize"), None);
        assert_eq!(hotkey_target_for_key("watchClipboard"), None);
        // The shortcut window/set_hotkey use the same mapping; the enum is stable.
        assert_eq!(hotkeys::Target::Capture.as_str(), "capture");
        assert_eq!(hotkeys::Target::Toggle.as_str(), "toggle");
        assert_eq!(hotkeys::Target::EditLatest.as_str(), "editLatest");
        assert_eq!(hotkeys::Target::Search.as_str(), "search");
        // Touch the unused-import-adjacent types so this test also covers their defaults.
        assert_eq!(CardSize::default(), CardSize::Medium);
        assert_eq!(KeepOnLine::default(), KeepOnLine::Fit);
    }

    /// W1.8: the newest card is the last row of the ascending `added_at` line; an empty line has
    /// nothing to open.
    #[test]
    fn newest_item_is_the_last_row_of_the_line() {
        use crate::item::{Item, ItemKind};
        let older = Item::new(
            PathBuf::from(r"C:\shots\older.png"),
            ItemKind::Capture,
            1,
            10,
            10,
        );
        let newer = Item::new(
            PathBuf::from(r"C:\shots\newer.mp4"),
            ItemKind::Recording,
            2,
            20,
            20,
        );
        let items = vec![older, newer.clone()];
        assert_eq!(
            newest_item(&items).map(|item| item.id.as_str()),
            Some(newer.id.as_str())
        );
        assert!(newest_item(&[]).is_none());
    }
}
