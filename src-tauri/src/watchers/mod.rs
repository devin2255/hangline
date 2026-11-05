//! Background watchers that turn new files/images into items on the line.
//!
//! Task 8 adds the folder watcher ([`dirs`]); Task 9 adds the clipboard listener ([`clipboard`]).
//! Both share [`crate::dedupe::Deduper`] and the `AppState` data layer, and both announce new items
//! through the same `state` event and Rust-only `reveal-request` (consumed by the reveal
//! controller in Task 11).
//!
//! [`start_all`] is called once from the Tauri `setup` hook after the settings are loaded. It
//! never fails startup: a missing/unreadable watched folder is logged and skipped, the user's
//! `Pictures`/`Videos` folders are never created, and only Hangline's own screenshot folder is
//! created (by [`crate::capture::screenshots_dir`], which Task 5 already owns). The clipboard
//! listener is started only when `watchClipboard` is on at startup.
//!
//! # Asset-protocol scope
//!
//! `tauri.conf.json` only statically allows the three default locations (`$LOCALDATA/Hangline/**`,
//! `$PICTURE/Screenshots/**`, `$VIDEO/Captures/**`). Every watched folder is also a thumbnail
//! source rendered through `convertFileSrc` (asset protocol), so [`allow_asset_dir`] adds it to the
//! runtime fs scope before its [`DirWatcher`] starts. [`start_one`] calls the helper for all
//! watched folders, including each `settings.extraFolders` entry, which is the only reason extra
//! folders' images load instead of being rejected with 403.
//!
//! The helper is `pub` for Task 15: before the settings UI starts watching a newly added folder it
//! must call [`allow_asset_dir`] (the helper warns and skips missing directories), so a file can
//! never be hung before its thumbnail is readable. Scope grants are additive and intentionally not
//! revoked when a folder is removed: items already on the line keep their thumbnails, and the
//! process restart restores the static `tauri.conf.json` scope. The call is idempotent — repeated
//! calls for the same directory are a no-op — and failures are logged, never fatal to startup.

pub mod clipboard;
pub mod dirs;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};

use tauri::{AppHandle, Manager};

use crate::app_state::AppState;
use crate::capture;
use dirs::{DirWatcher, Opts};

/// The watchers started at bootstrap and restarted by `set_setting` (Task 15).
///
/// Owning them keeps them alive for the whole app lifetime; [`shutdown`](Watchers::shutdown) (the
/// tray Quit action and the `RunEvent::Exit` hook) drops every [`DirWatcher`] and the clipboard
/// listener, which stops event delivery and joins each worker thread. Both collections live
/// behind mutexes so the watch switches can start/stop them at runtime without restarting the app.
pub struct Watchers {
    running: Mutex<Vec<DirWatcher>>,
    /// The clipboard listener, present only while `watchClipboard` is on.
    clipboard: Mutex<Option<clipboard::ClipboardWatcher>>,
}

impl Watchers {
    /// Number of active folder watchers.
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    /// Whether no folder could be watched (e.g. all folders are missing).
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Whether the clipboard listener is currently running.
    pub fn clipboard_running(&self) -> bool {
        self.clipboard_lock().is_some()
    }

    /// Locks the watcher list; a poisoned mutex is recovered (the list is still safe to drop).
    fn lock(&self) -> MutexGuard<'_, Vec<DirWatcher>> {
        self.running.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Locks the clipboard-listener slot; a poisoned mutex is recovered.
    fn clipboard_lock(&self) -> MutexGuard<'_, Option<clipboard::ClipboardWatcher>> {
        self.clipboard
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Stops every folder watcher: dropping a [`DirWatcher`] stops notify and joins the worker.
    fn stop_folders(&self) -> usize {
        let stopped: Vec<DirWatcher> = {
            let mut guard = self.lock();
            std::mem::take(&mut *guard)
        };
        let count = stopped.len();
        drop(stopped);
        count
    }

    /// Stops the clipboard listener if it is running (idempotent).
    fn stop_clipboard(&self) {
        let listener = self.clipboard_lock().take();
        if let Some(listener) = listener {
            drop(listener);
            log::info!("watchers: clipboard listener stopped (watchClipboard = false)");
        }
    }

    /// Stops the clipboard listener and every folder watcher; safe to call twice (Quit + exit hook).
    pub fn shutdown(&self) {
        let clipboard = self.clipboard_running();
        log::info!(
            "watchers: stopping {} folder watcher(s), clipboard listener {}",
            self.len(),
            if clipboard { "on" } else { "off" }
        );
        self.stop_clipboard();
        let stopped = self.stop_folders();
        log::info!("watchers: {stopped} folder watcher(s) stopped");
    }
}

impl Drop for Watchers {
    fn drop(&mut self) {
        let clipboard = self
            .clipboard
            .lock()
            .map(|guard| guard.is_some())
            .unwrap_or(false);
        log::info!(
            "watchers: dropping {} folder watcher(s), clipboard listener {}",
            self.running
                .lock()
                .map(|guard| guard.len())
                .unwrap_or(0),
            if clipboard { "on" } else { "off" }
        );
        // The fields are dropped after this body: the clipboard watcher posts WM_CLOSE and joins
        // its listener thread, each DirWatcher stops notify and joins its debounce worker.
    }
}

/// Stops every running watcher (tray Quit / `RunEvent::Exit`); a no-op before setup managed them.
pub fn shutdown(app: &AppHandle) {
    if let Some(watchers) = app.try_state::<Watchers>() {
        watchers.shutdown();
    }
}

/// Restarts the folder watchers from the current settings (Task 15 watch switches / extra folders).
///
/// The whole folder set is stopped and rebuilt: it is the simplest correct behaviour for the three
/// switches plus the folder list, and folder events are rare (a menu click). New `extraFolders`
/// entries are added to the asset-protocol scope by [`start_one`] *before* their watcher starts;
/// removing a folder only stops its watcher — the asset scope is never revoked (Task 10 contract:
/// cards already on the line keep their thumbnails, and dynamic grants are not persisted anyway).
pub fn restart_folder_watchers(app: &AppHandle) {
    let Some(watchers) = app.try_state::<Watchers>() else {
        log::warn!("watchers: restart requested before the watchers were managed; ignoring");
        return;
    };
    let settings = app.state::<AppState>().snapshot().settings;
    let stopped = watchers.stop_folders();
    let mut running = Vec::new();
    start_all_folders(app, &settings, &mut running);
    let started = running.len();
    *watchers.lock() = running;
    log::info!(
        "watchers: folder watchers restarted ({stopped} stopped, {started} active; \
         watchSystemScreenshots={}, watchRecordings={}, extraFolders={})",
        settings.watch_system_screenshots,
        settings.watch_recordings,
        settings.extra_folders.len()
    );
}

/// Starts the clipboard listener if it is not already running (idempotent).
pub fn start_clipboard(app: &AppHandle) {
    let Some(watchers) = app.try_state::<Watchers>() else {
        log::warn!("watchers: clipboard start requested before the watchers were managed");
        return;
    };
    let mut slot = watchers.clipboard_lock();
    if slot.is_some() {
        log::info!("watchers: clipboard listener is already running; no restart needed");
        return;
    }
    match clipboard::start(app) {
        Ok(listener) => {
            *slot = Some(listener);
            log::info!("watchers: clipboard listener started (watchClipboard = true)");
        }
        Err(error) => log::warn!("watchers: cannot start the clipboard listener: {error:#}"),
    }
}

/// Stops the clipboard listener if it is running (idempotent).
pub fn stop_clipboard(app: &AppHandle) {
    if let Some(watchers) = app.try_state::<Watchers>() {
        watchers.stop_clipboard();
    }
}

/// Directories already added to the asset-protocol scope by [`allow_asset_dir`].
///
/// Keys are normalized (component-collected, lowercase) because Windows paths are
/// case-insensitive; the set is the helper's idempotency guard.
static ASSET_SCOPE_DIRS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

/// Extends the asset-protocol scope to `path` recursively, so the panel's `convertFileSrc`
/// thumbnails can read files from a watched folder.
///
/// This is the runtime half of the static `assetProtocol.scope` in `tauri.conf.json` (design spec
/// §4.4): without it an image from `settings.extraFolders` loads through the asset protocol only
/// to be rejected (403) and the card falls back to a filename tile. A missing directory is logged
/// and skipped (the caller must not create user folders). The helper is idempotent per normalized
/// path, logs instead of returning errors, and retries on a later call if `allow_directory` failed.
///
/// `pub` is the Task 15 contract: the settings UI must call this for every newly watched extra
/// folder *before* its watcher starts (same order as [`start_one`]).
pub fn allow_asset_dir(app: &AppHandle, path: &Path) {
    if !path.is_dir() {
        log::warn!(
            "watchers: {} is not an existing directory; not adding it to the asset protocol scope",
            path.display()
        );
        return;
    }
    let key = asset_scope_key(path);
    // Hold the lock across `allow_directory` so two concurrent calls for the same folder cannot
    // both update the scope (each update also emits a path-allowed event to the frontend).
    let mut allowed = asset_scope_dirs()
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if !allowed.insert(key.clone()) {
        log::debug!(
            "watchers: asset protocol already allows {}; skipping",
            path.display()
        );
        return;
    }
    match app.asset_protocol_scope().allow_directory(path, true) {
        Ok(()) => log::info!(
            "watchers: asset protocol allows {} (recursive)",
            path.display()
        ),
        Err(error) => {
            // Do not record a failed attempt: a later call (Task 15 restart) may succeed.
            allowed.remove(&key);
            log::warn!(
                "watchers: cannot add {} to the asset protocol scope: {error:#}",
                path.display()
            );
        }
    }
}

/// Returns the idempotency set, creating it on first use.
fn asset_scope_dirs() -> &'static Mutex<HashSet<String>> {
    ASSET_SCOPE_DIRS.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Normalizes a directory path for the idempotency set: components are collected (so trailing
/// separators and `.` components collapse) and lowercased (Windows path comparison).
fn asset_scope_key(path: &Path) -> String {
    let normalized: PathBuf = path.components().collect();
    normalized.to_string_lossy().to_ascii_lowercase()
}

/// Starts every configured folder watcher plus the clipboard listener, logging (never failing) on
/// unavailable folders or a listener that cannot start.
///
/// The watch set is design spec 3.2/4.8: system screenshots, recordings, Hangline's own captures
/// and up to three `extraFolders`. The two watch switches from the settings gate the system
/// folders; our own folder is always watched (it is how Task 16's captures reach the line). The
/// clipboard listener (design spec 3.2 source C) is gated by `watchClipboard`. Task 15 owns the
/// restart API: [`restart_folder_watchers`], [`start_clipboard`] and [`stop_clipboard`] re-run this
/// configuration when the user toggles a switch in the tray.
pub fn start_all(app: &AppHandle) -> Watchers {
    let settings = app.state::<AppState>().snapshot().settings;
    let mut watchers = Vec::new();
    start_all_folders(app, &settings, &mut watchers);

    let clipboard = if settings.watch_clipboard {
        match clipboard::start(app) {
            Ok(watcher) => Some(watcher),
            Err(error) => {
                log::warn!("watchers: cannot start the clipboard listener: {error:#}");
                None
            }
        }
    } else {
        log::info!("watchers: clipboard watching disabled in the settings");
        None
    };

    log::info!(
        "watchers: {} folder watcher(s) active, clipboard listener {}",
        watchers.len(),
        if clipboard.is_some() { "on" } else { "off" }
    );
    let watchers = Watchers {
        running: Mutex::new(watchers),
        clipboard: Mutex::new(clipboard),
    };
    if watchers.is_empty() {
        log::warn!("watchers: no folders are being watched (all missing or denied)");
    }
    watchers
}

/// Starts the folder watchers for one settings snapshot into `watchers`.
///
/// Shared by startup ([`start_all`]) and the runtime restart API so both apply exactly the same
/// folder set, switch gating and duplicate/asset-scope rules.
fn start_all_folders(app: &AppHandle, settings: &crate::settings::Settings, watchers: &mut Vec<DirWatcher>) {
    let mut seen: Vec<String> = Vec::new();

    if settings.watch_system_screenshots {
        match user_folder("Pictures", "Screenshots") {
            Some(folder) => start_one(
                app,
                &folder,
                Opts::system_screenshots(),
                watchers,
                &mut seen,
            ),
            None => log::warn!("watchers: USERPROFILE is not set; system screenshots are off"),
        }
    } else {
        log::info!("watchers: system screenshots disabled in the settings");
    }

    if settings.watch_recordings {
        match recordings_folder() {
            Some(folder) => start_one(app, &folder, Opts::recordings(), watchers, &mut seen),
            None => log::warn!("watchers: USERPROFILE is not set; recordings are off"),
        }
    } else {
        log::info!("watchers: recordings disabled in the settings");
    }

    match capture::screenshots_dir() {
        Ok(folder) => start_one(app, &folder, Opts::own_captures(), watchers, &mut seen),
        Err(error) => log::warn!("watchers: own screenshot folder unavailable: {error:#}"),
    }

    for extra in &settings.extra_folders {
        start_one(
            app,
            Path::new(extra),
            Opts::watch_any_image(),
            watchers,
            &mut seen,
        );
    }
}

/// The recordings folder, with a debug-only test hook.
///
/// `HANG_RECORDINGS_DIR` relocates only this watcher (debug builds). The isolated evidence runs
/// cannot override `USERPROFILE`: tauri-plugin-log resolves its log directory through the shell
/// known folders and fails startup (`PluginInitialization("log", "unknown path")`) when the
/// profile environment is inconsistent. Release builds ignore the variable and use the literal
/// `%USERPROFILE%\Videos\Captures`.
fn recordings_folder() -> Option<PathBuf> {
    #[cfg(debug_assertions)]
    if let Some(dir) = std::env::var_os("HANG_RECORDINGS_DIR").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    user_folder("Videos", "Captures")
}

/// `%USERPROFILE%\<parent>\<leaf>` (the literal paths from design spec 4.8).
fn user_folder(parent: &str, leaf: &str) -> Option<PathBuf> {
    let profile = std::env::var_os("USERPROFILE").filter(|value| !value.is_empty())?;
    Some(PathBuf::from(profile).join(parent).join(leaf))
}

/// Starts one watcher, skipping missing folders, duplicates and watch errors.
///
/// The folder is added to the asset-protocol scope ([`allow_asset_dir`]) before the watcher
/// starts, so thumbnails of its files load through `convertFileSrc`.
///
/// Duplicate folders are dropped case-insensitively (Windows paths), so configuring the system
/// screenshot folder as an extra folder cannot produce two watchers for the same directory.
fn start_one(
    app: &AppHandle,
    folder: &Path,
    opts: Opts,
    watchers: &mut Vec<DirWatcher>,
    seen: &mut Vec<String>,
) {
    let key = folder.to_string_lossy().to_ascii_lowercase();
    if seen.iter().any(|existing| *existing == key) {
        log::info!(
            "watchers: {} is already watched; skipping the duplicate",
            folder.display()
        );
        return;
    }
    if !folder.is_dir() {
        log::warn!(
            "watchers: {} does not exist; not watching it (folder creation is the user's call)",
            folder.display()
        );
        return;
    }
    // The panel renders every item's thumbnail through the asset protocol. Allowing the folder
    // before the watcher starts means the first emitted item can never race an unscoped 403.
    allow_asset_dir(app, folder);
    seen.push(key);
    match dirs::start(app, folder, opts) {
        Ok(watcher) => watchers.push(watcher),
        Err(error) => log::warn!("watchers: cannot watch {}: {error:#}", folder.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_folder_joins_the_profile_without_creating_anything() {
        let folder = user_folder("Pictures", "Screenshots").expect("USERPROFILE is set on Windows");
        let profile = PathBuf::from(std::env::var_os("USERPROFILE").expect("USERPROFILE"));
        assert_eq!(folder, profile.join("Pictures").join("Screenshots"));
        assert_eq!(
            folder.parent().and_then(Path::file_name),
            Some(std::ffi::OsStr::new("Pictures"))
        );
    }

    #[test]
    fn asset_scope_keys_dedupe_case_and_trailing_separators() {
        // Task 15 may pass the same folder with different casing or a trailing separator; the
        // idempotency set must recognize it as already allowed.
        let plain = Path::new(r"C:\Users\Example\Pictures\Shots");
        let noisy = Path::new(r"c:\users\example\pictures\shots\");
        assert_eq!(asset_scope_key(plain), asset_scope_key(noisy));
        assert_eq!(
            asset_scope_key(plain),
            r"c:\users\example\pictures\shots".to_string()
        );
        assert_ne!(
            asset_scope_key(plain),
            asset_scope_key(Path::new(r"C:\Users\Example\Pictures\Other"))
        );
    }
}
