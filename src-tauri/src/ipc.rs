//! Tauri commands exposing the data layer (design spec 4.4).
//!
//! The frontend never owns state: it calls [`get_app_state`] once on startup and then follows the
//! `state` event. [`set_setting`] is the only mutating command in this task; the item commands
//! (copy/take-down/capture) arrive in later tasks and reuse [`crate::app_state::AppState`].

use serde_json::Value;
use tauri::{AppHandle, State};

use crate::app_state::{AppState, PublicState};
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
/// - `cardSize`/`keepOnLine` re-apply the panel window geometry (Task 14).
#[tauri::command]
pub fn set_setting(
    key: String,
    value: Value,
    app: tauri::AppHandle,
) -> Result<PublicState, String> {
    crate::actions::apply(&app, &key, value)
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

#[cfg(test)]
mod tests {
    // The commands are thin wrappers over `AppState` (covered by its unit tests) and need a live
    // Tauri runtime for `State`/`AppHandle`; the end-to-end handshake is verified by the Task 14
    // runtime evidence. The geometry-affecting setting predicate is unit-tested once in
    // `settings::tests::geometry_affecting_settings_are_card_size_and_keep_on_line`; the duplicate
    // copy that used to live here was removed by the Task 14 review fix.
}
