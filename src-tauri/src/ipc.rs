//! Tauri commands exposing the data layer (design spec 4.4).
//!
//! The frontend never owns state: it calls [`get_app_state`] once on startup and then follows the
//! `state` event. [`set_setting`] mutates the settings; [`set_item_note`]/[`set_item_tags`] are the
//! Wave 2 item-metadata writes (one persisted mutation + one broadcast each, like every other
//! `AppState` mutator). [`ocr_status`]/[`build_ocr_index`]/[`search_ocr`] expose the Wave 2.5 local
//! OCR index PoC (`crate::ocr`); the build command is async + blocking-worker because OCR is CPU/COM
//! work and must never block the UI.

use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::Value;
use tauri::{AppHandle, Manager, State};

use crate::app_state::{self, AppState, PublicState};
use crate::geometry::MonitorInfo;
use crate::panel::{GeometryTarget, PanelGeometry};

/// Returns the full application state (`items + settings + version`).
///
/// Called by the frontend on startup and after a reconnect; the `state` event keeps it current.
#[tauri::command]
pub fn get_app_state(state: State<'_, AppState>) -> PublicState {
    let snapshot = state.snapshot();
    log::info!(
        "ipc: get_app_state -> {} item(s), version {}",
        snapshot.items.len(),
        snapshot.version
    );
    snapshot
}

/// Validates and persists one setting, then runs its side effects and broadcasts the new state.
///
/// Keys are the camelCase spec 3.6 names; invalid keys/values return the validation message
/// unchanged (the settings UI shows it next to the control). The command is a thin wrapper over
/// [`crate::actions::apply`] so the tray, the shortcut window and the folder picker share exactly
/// one persistence + side-effect + rollback path:
///
/// - watch switches/clipboard/`extraFolders` start/stop the real watchers (Task 15);
/// - `captureHotkey`/`toggleHotkey` re-register the global shortcut, restoring the previous value
///   on a conflict (design spec §3.11);
/// - `startAtLogin` registers/unregisters the Windows login item;
/// - `language` rebuilds the tray menu;
/// - `cardSize`/`keepOnLine` re-apply the panel window geometry (Task 14);
/// - `keepForDays` starts the immediate age-retention sweep (expired cards leave the line, expired
///   Hangline-owned screenshots are recycled; watched/system/recording files are never deleted).
#[tauri::command]
pub fn set_setting(
    key: String,
    value: Value,
    app: tauri::AppHandle,
) -> Result<PublicState, String> {
    crate::actions::apply(&app, &key, value)
}

/// Replaces one card's note (Wave 2 W2.1).
///
/// Validation (`≤ 2000` characters), the single revision bump and the single `state.json` write
/// live in [`AppState::set_note`]; this command only maps the error and broadcasts the new snapshot
/// once. An unchanged note is a no-op (no bump, no broadcast); a stale id returns a diagnostic error
/// the search window can show inline.
#[tauri::command]
pub fn set_item_note(app: AppHandle, id: String, note: String) -> Result<(), String> {
    let state = app.state::<AppState>();
    let changed = state.set_note(&id, &note).map_err(|error| {
        log::warn!("ipc: set_item_note rejected for {id}: {error:#}");
        error.to_string()
    })?;
    if changed {
        log::info!("ipc: set_item_note updated {id} ({} characters)", note.chars().count());
        let _ = app_state::emit_state(&app);
    }
    Ok(())
}

/// Replaces one card's tags (Wave 2 W2.1).
///
/// The list is trimmed/de-duplicated/limited by [`AppState::set_tags`] (`≤ 32` characters per tag,
/// `≤ 20` tags); one revision bump + one persist, then one broadcast. An unchanged normalized list
/// is a no-op.
#[tauri::command]
pub fn set_item_tags(app: AppHandle, id: String, tags: Vec<String>) -> Result<(), String> {
    let state = app.state::<AppState>();
    let changed = state.set_tags(&id, &tags).map_err(|error| {
        log::warn!("ipc: set_item_tags rejected for {id}: {error:#}");
        error.to_string()
    })?;
    if changed {
        log::info!(
            "ipc: set_item_tags updated {id} ({} tag(s) stored)",
            state.item(&id).map(|item| item.tags.len()).unwrap_or(0)
        );
        let _ = app_state::emit_state(&app);
    }
    Ok(())
}

/// Returns the monitors of the virtual desktop (design spec 4.4); a pure query.
///
/// Task 14 split the old side effect: this command only enumerates monitors
/// ([`crate::geometry::monitors`]) and never moves or resizes the panel. Use
/// [`sync_panel_geometry`] for the startup resync/geometry application.
#[tauri::command]
pub fn get_monitors() -> Vec<MonitorInfo> {
    let monitors = crate::geometry::monitors();
    log::info!("ipc: get_monitors -> {} monitor(s)", monitors.len());
    monitors
}

/// Re-applies the panel geometry to the monitor the panel is already on and returns the result.
///
/// This is the explicit resync command (startup, and any frontend caller after a display change).
/// It deliberately re-fits [`GeometryTarget::CurrentPanel`], not the pointer's monitor: the user
/// invoking a settings action from another display must not move the line. Only the reveal
/// controller's debounced follow targets [`GeometryTarget::Cursor`]. Both use the same
/// [`crate::panel::sync_panel_geometry`] function, so there is exactly one geometry owner in Rust;
/// this command is a thin wrapper for the frontend.
#[tauri::command]
pub fn sync_panel_geometry(app: AppHandle) -> PanelGeometry {
    let geometry = crate::panel::sync_panel_geometry(&app, GeometryTarget::CurrentPanel);
    log::info!(
        "ipc: sync_panel_geometry (current panel) -> {} monitor(s), target {:?}, moved={}, \
         size {:?}",
        geometry.monitors.len(),
        geometry
            .target
            .as_ref()
            .map(|monitor| monitor.id.as_str()),
        geometry.moved,
        geometry.size
    );
    geometry
}

/// Reports the Windows OCR availability (W2.5 PoC): engine, language, installed languages, error.
#[tauri::command]
pub fn ocr_status() -> crate::ocr::OcrAvailability {
    let availability = crate::ocr::availability();
    log::info!(
        "ipc: ocr_status -> available={}, language={:?}, languages={:?}",
        availability.available,
        availability.language,
        availability.languages
    );
    availability
}

/// Single-flight guard: at most one OCR index build runs at a time (W2.5).
static OCR_BUILD_RUNNING: AtomicBool = AtomicBool::new(false);

/// Clears the single-flight claim when a build ends (including early returns and panics).
struct OcrBuildGuard;

impl OcrBuildGuard {
    fn acquire() -> Result<Self, String> {
        if OCR_BUILD_RUNNING.swap(true, Ordering::AcqRel) {
            return Err("an OCR index build is already running".to_string());
        }
        Ok(Self)
    }
}

impl Drop for OcrBuildGuard {
    fn drop(&mut self) {
        OCR_BUILD_RUNNING.store(false, Ordering::Release);
    }
}

/// Builds/updates the local OCR index (W2.5 PoC); async + blocking worker + single-flight.
///
/// `force` re-runs OCR for unchanged hashes; `limit` caps the number of attempts (`0`/absent = no
/// cap). Recognition is CPU/COM work, so it runs on `spawn_blocking`, never on the UI thread or the
/// async reactor; a second concurrent call returns a diagnostic error instead of stacking work.
#[tauri::command]
pub async fn build_ocr_index(
    app: AppHandle,
    force: Option<bool>,
    limit: Option<usize>,
) -> Result<crate::ocr::OcrBuildReport, String> {
    let guard = OcrBuildGuard::acquire()?;
    let handle = app.clone();
    let joined = tauri::async_runtime::spawn_blocking(move || {
        // The claim lives until the worker has finished writing the index.
        let _guard = guard;
        crate::ocr::build(&handle, force.unwrap_or(false), limit.unwrap_or(0))
    })
    .await
    .map_err(|error| format!("build_ocr_index task failed: {error}"))?;
    joined.map_err(|error| {
        log::warn!("ipc: build_ocr_index failed: {error}");
        error
    })
}

/// Searches the persisted OCR index (W2.5 PoC): case-insensitive substring → `{ id, excerpt }`.
///
/// A missing/corrupt index simply yields no hits (the cache is rebuilt by `build_ocr_index`).
#[tauri::command]
pub fn search_ocr(query: String) -> Vec<crate::ocr::OcrHit> {
    let hits = crate::ocr::search(&query);
    log::info!("ipc: search_ocr({query:?}) -> {} hit(s)", hits.len());
    hits
}

#[cfg(test)]
mod tests {
    // The commands are thin wrappers over `AppState` (covered by its unit tests) and need a live
    // Tauri runtime for `State`/`AppHandle`; the end-to-end handshake is verified by the Task 14
    // runtime evidence. The Wave 2 validation/normalization rules (`set_item_note`/`set_item_tags`)
    // are unit-tested in `app_state::tests` (`validate_note`, `normalize_tags`, `set_note`,
    // `set_tags`), including the "one revision bump per mutation" contract. The Wave 2.5 OCR
    // commands wrap `crate::ocr`, whose pure logic (index merge/parse/excerpt/search + the
    // `--ocr-probe` argument parsing in `main.rs`) is unit-tested there; real OCR calls are
    // `#[ignore]`d and exercised by the probe. The geometry-affecting setting predicate is
    // unit-tested once in
    // `settings::tests::geometry_affecting_settings_are_card_size_and_keep_on_line`; the duplicate
    // copy that used to live here was removed by the Task 14 review fix.
}
