//! Age-based retention (`keepForDays`, design extension): the background sweep and file cleanup.
//!
//! The policy is a settings value (`Forever` / 7 / 30 / 90 days). `AppState::trim_expired` owns the
//! state half (drop expired rows, persist, bump the revision); this module owns the lifecycle half:
//!
//! - [`spawn`] starts one named background thread at startup: an immediate sweep, then one sweep
//!   per [`SWEEP_INTERVAL`]. Nothing runs on the UI thread and the state lock is only held for the
//!   in-memory trim + persist.
//! - [`sweep_async`] runs a one-shot sweep off the UI thread; `actions::apply` schedules it as the
//!   soft [`crate::actions::SettingAction::SweepRetention`] side effect of a `keepForDays` write, so
//!   shortening the window takes effect immediately instead of at the next hourly tick.
//! - After the state trim, [`cleanup_plan`] decides which expired items may have their file
//!   recycled: only Hangline-owned screenshots ([`recyclable_files`]: `Capture`/`Clipboard` items
//!   under the screenshot directory, reusing [`trash::take_down_action`]'s "inbox mode" rule).
//!   Watched-folder, system-screenshot and recording files are never touched; a recycle failure
//!   leaves the file in place and only logs a warning.
//!
//! The sweep also broadcasts one `state` snapshot when it removed anything, so the panel and the
//! tray drop the expired cards immediately.

use std::path::{Path, PathBuf};
use std::time::Duration;

use tauri::{AppHandle, Manager};

use crate::app_state::{self, AppState};
use crate::item::{self, Item, ItemKind};
use crate::trash::{self, TakeDownAction};

/// How often the running app re-checks card ages. The policy is day-granularity, so an hourly
/// sweep is plenty and the wake-up cost is one lock + list scan.
pub const SWEEP_INTERVAL: Duration = Duration::from_secs(60 * 60);

/// What one sweep does with the expired rows.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct CleanupPlan {
    /// Existing Hangline-owned screenshots to recycle into the Recycle Bin.
    pub recyclable: Vec<PathBuf>,
    /// Expired rows whose file is already gone (nothing to recycle, nothing to delete).
    pub missing: usize,
    /// Expired rows whose file must be kept (watched folder, system screenshot, recording).
    pub kept: usize,
}

/// Files of `expired` that may be recycled into the Recycle Bin.
///
/// Two independent guards, in this order: the item must be a Hangline-owned *kind*
/// (`Capture`/`Clipboard` — the only kinds that can legitimately live in the screenshot directory)
/// and the shared [`trash::take_down_action`] decision table must say `Recycle` for its path. A
/// redirected or hand-edited source can therefore never trick the sweep into recycling a system
/// screenshot, a recording or a watched-folder file, even if it points into our directory. A file
/// that is already gone is skipped (nothing to recycle).
pub fn recyclable_files(expired: &[Item], screenshots_dir: &Path) -> Vec<PathBuf> {
    expired
        .iter()
        .filter(|item| matches!(item.kind, ItemKind::Capture | ItemKind::Clipboard))
        .filter(|item| {
            trash::take_down_action(item.kind, &item.path, screenshots_dir)
                == TakeDownAction::Recycle
        })
        .filter(|item| item.path.is_file())
        .map(|item| item.path.clone())
        .collect()
}

/// Splits the expired rows into "recycle these", "already gone" and "keep the file" buckets.
///
/// Pure so the sweep's reporting (and the guarantee that non-owned files are never recycled) can be
/// unit-tested without touching the shell or `%LOCALAPPDATA%`.
pub fn cleanup_plan(expired: &[Item], screenshots_dir: &Path) -> CleanupPlan {
    let recyclable = recyclable_files(expired, screenshots_dir);
    let missing = expired.iter().filter(|item| !item.path.is_file()).count();
    let kept = expired.len() - recyclable.len() - missing;
    CleanupPlan {
        recyclable,
        missing,
        kept,
    }
}

/// Starts the named retention worker: one immediate sweep, then one per [`SWEEP_INTERVAL`].
///
/// The thread is detached on purpose: it owns no window and does no teardown work, so the process
/// exit does not need to join it (same lifetime model as the thumbnail backfill thread).
pub fn spawn(app: &AppHandle) {
    let handle = app.clone();
    let spawned = std::thread::Builder::new()
        .name("hangline-retention".to_string())
        .spawn(move || {
            sweep(&handle);
            loop {
                std::thread::sleep(SWEEP_INTERVAL);
                sweep(&handle);
            }
        });
    if let Err(error) = spawned {
        log::error!("retention: cannot spawn the retention worker: {error}");
    }
}

/// Runs one sweep on a short-lived background thread (the `SweepRetention` settings side effect).
pub fn sweep_async(app: &AppHandle) {
    let handle = app.clone();
    let spawned = std::thread::Builder::new()
        .name("hangline-retention-now".to_string())
        .spawn(move || sweep(&handle));
    if let Err(error) = spawned {
        log::warn!("retention: cannot spawn the immediate sweep thread: {error}");
    }
}

/// One sweep: trim the state, broadcast it, then recycle the expired Hangline-owned screenshots.
///
/// Runs on a background thread only. The state lock is released before the broadcast and before
/// any Recycle Bin/shell work; a failure in one half is logged and never aborts the other.
fn sweep(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        log::debug!("retention: app state is not managed yet; skipping the sweep");
        return;
    };
    let keep_for_days = state.snapshot().settings.keep_for_days;
    let expired = match state.trim_expired(item::now_ms()) {
        Ok(expired) => expired,
        Err(error) => {
            log::error!("retention: cannot persist the age-trimmed item list: {error:#}");
            // The in-memory list already changed before the persist failed (same contract as
            // `take_down`): publish it, so the panel never keeps rendering cards the state lost.
            if app_state::emit_state(app).is_none() {
                log::warn!("retention: could not broadcast the partially-trimmed state");
            }
            return;
        }
    };
    if expired.is_empty() {
        return;
    }
    log::info!(
        "retention: {} card(s) left the line by age; keepForDays = {keep_for_days:?}",
        expired.len()
    );
    if app_state::emit_state(app).is_none() {
        log::warn!("retention: could not broadcast the age-trimmed state");
    }

    let screenshots_dir = match crate::capture::screenshots_dir() {
        Ok(dir) => dir,
        Err(error) => {
            log::warn!(
                "retention: cannot resolve the screenshot directory ({error:#}); \
                 keeping every expired file on disk"
            );
            return;
        }
    };
    let plan = cleanup_plan(&expired, &screenshots_dir);
    for path in &plan.recyclable {
        match trash::recycle(path) {
            Ok(()) => log::info!("retention: recycled expired screenshot {}", path.display()),
            Err(error) => log::warn!(
                "retention: cannot recycle {} ({error:#}); the file stays on disk",
                path.display()
            ),
        }
    }
    if plan.missing > 0 {
        log::info!(
            "retention: {} expired file(s) were already gone; nothing to recycle",
            plan.missing
        );
    }
    if plan.kept > 0 {
        log::info!(
            "retention: kept {} expired file(s) outside Hangline's screenshot directory \
             (inbox/system/recording files are never deleted)",
            plan.kept
        );
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use super::*;

    /// Scratch directory under `target/` (git-ignored); never touches `%LOCALAPPDATA%`.
    fn test_dir(tag: &str) -> PathBuf {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("retention-tests")
            .join(format!("{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create the retention test dir");
        dir
    }

    /// Creates a real file so the recycle plan can see it exists.
    fn write_file(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, b"item bytes").expect("create the test file");
        path
    }

    /// Builds an item with a deterministic id/path/kind.
    fn item(id: &str, path: &Path, kind: ItemKind, added_at: i64) -> Item {
        Item {
            id: id.to_string(),
            path: path.to_path_buf(),
            thumb_path: None,
            kind,
            hash: 7,
            width: 10,
            height: 20,
            created_at: 1,
            added_at,
            updated_at: added_at,
            note: String::new(),
            tags: Vec::new(),
        }
    }

    /// The recycle plan is the "inbox mode" rule again: Hangline-owned capture/clipboard screenshots
    /// are recycled, watched-folder/system-screenshot/recording files and already-missing files are
    /// not. The system/recording fixtures point *inside* the screenshot directory on purpose: the
    /// kind allowlist must stop them even though the path rule alone would say `Recycle`.
    #[test]
    fn recyclable_files_only_include_owned_screenshots_that_still_exist() {
        let base = test_dir("recycle-plan");
        let own_dir = base.join("Screenshots");
        let other_dir = base.join("Watched");
        fs::create_dir_all(&own_dir).expect("create the own dir");
        fs::create_dir_all(&other_dir).expect("create the watched dir");

        let own_capture = write_file(&own_dir, "own.png");
        let own_clipboard = write_file(&own_dir, "Clipboard 2026-11-05.png");
        let watched = write_file(&other_dir, "watched.png");
        let system_outside = write_file(&base, "system.png");
        let recording_outside = write_file(&base, "capture.mp4");
        let system_inside = write_file(&own_dir, "system-inside.png");
        let recording_inside = write_file(&own_dir, "recording-inside.mp4");
        let missing_own = own_dir.join("gone.png");
        let expired = vec![
            item("own", &own_capture, ItemKind::Capture, 1),
            item("clipboard", &own_clipboard, ItemKind::Clipboard, 2),
            item("watched", &watched, ItemKind::WatchedFolder, 3),
            item("system", &system_outside, ItemKind::SystemScreenshot, 4),
            item("recording", &recording_outside, ItemKind::Recording, 5),
            item("system-inside", &system_inside, ItemKind::SystemScreenshot, 6),
            item("recording-inside", &recording_inside, ItemKind::Recording, 7),
            item("missing", &missing_own, ItemKind::Capture, 8),
        ];

        assert_eq!(
            recyclable_files(&expired, &own_dir),
            vec![own_capture.clone(), own_clipboard.clone()],
            "only existing Hangline-owned capture/clipboard screenshots may be recycled"
        );

        let plan = cleanup_plan(&expired, &own_dir);
        assert_eq!(plan.recyclable, vec![own_capture, own_clipboard]);
        assert_eq!(plan.missing, 1, "the missing screenshot needs no recycle call");
        assert_eq!(
            plan.kept, 5,
            "watched/system/recording rows (inside or outside the dir) keep their files"
        );

        let _ = fs::remove_dir_all(base);
    }
}
