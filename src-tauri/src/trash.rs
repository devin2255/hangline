//! Shell file operations: recycle into the Windows Recycle Bin and open with the default app.
//!
//! Deletion policy (design spec §3.1 / §4.6 E): only files that Hangline owns (inside
//! `%LOCALAPPDATA%\Hangline\Screenshots`) may be recycled. Items coming from a watched folder
//! ("inbox" mode) are only taken off the line; their file belongs to the user's folder. The pure
//! [`take_down_action`] decision table keeps that rule testable without touching the shell.
//!
//! `IFileOperation` with `FOF_ALLOWUNDO | FOFX_RECYCLEONDELETE | FOF_NOCONFIRMATION` puts the file
//! in the Recycle Bin (undoable), never permanently deletes it.

use std::ffi::OsString;
use std::os::windows::ffi::OsStrExt;
use std::path::{Component, Path};

use anyhow::{bail, Context};
use windows::core::{w, PCWSTR, HRESULT};
use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::Shell::{
    FileOperation, IFileOperation, IShellItem, SHCreateItemFromParsingName, ShellExecuteW,
    FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOFX_RECYCLEONDELETE,
};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

use crate::item::ItemKind;

/// What taking one card down is allowed to do to its file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TakeDownAction {
    /// Remove the card only; the file stays where it is.
    OffLineOnly,
    /// Recycle the file into the Recycle Bin, then remove the card.
    Recycle,
}

/// Decides whether `path` may be recycled when its card is taken down.
///
/// Only files below `screenshots_dir` are Hangline's own; everything else (system screenshot
/// folders, recordings, watched folders) only leaves the line. Watched-folder cards are always
/// offline-only, even if the user configured a folder inside Hangline's own directory: that folder
/// is the user's inbox, not Hangline-owned storage (spec §3.1 "inbox 模式...只下线不删").
///
/// The comparison is lexical, case-insensitive and resolves `.`/`..` components, so a stored path
/// cannot escape the directory by spelling (Windows paths are case-insensitive). The function
/// stays pure so the decision table can be unit-tested without a file system.
pub fn take_down_action(kind: ItemKind, path: &Path, screenshots_dir: &Path) -> TakeDownAction {
    if kind == ItemKind::WatchedFolder {
        return TakeDownAction::OffLineOnly;
    }
    if !path.is_absolute() || !screenshots_dir.is_absolute() {
        return TakeDownAction::OffLineOnly;
    }
    let file = folded_components(path);
    let own = folded_components(screenshots_dir);
    if file.len() > own.len() && file.starts_with(&own) {
        TakeDownAction::Recycle
    } else {
        TakeDownAction::OffLineOnly
    }
}

/// The lexical components of `path`, resolving `.` and `..` without touching the file system.
fn normalized_components(path: &Path) -> Vec<OsString> {
    let mut components: Vec<OsString> = Vec::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => components.push(prefix.as_os_str().to_os_string()),
            Component::RootDir => components.push(OsString::from("\\")),
            Component::CurDir => {}
            Component::ParentDir => {
                // Never pop the root (or the "C:" prefix); a leading `..` on a relative path is
                // irrelevant here because take_down_action rejects non-absolute paths.
                if components
                    .last()
                    .is_some_and(|last| last != "\\" && !last.to_string_lossy().ends_with(':'))
                {
                    components.pop();
                }
            }
            Component::Normal(name) => components.push(name.to_os_string()),
        }
    }
    components
}

/// Case-folded components for the Windows case-insensitive comparison.
fn folded_components(path: &Path) -> Vec<String> {
    normalized_components(path)
        .iter()
        .map(|component| component.to_string_lossy().to_ascii_lowercase())
        .collect()
}

/// Applies the "file is already gone" fallback to a take-down decision.
///
/// A recycle request is only executable while the file exists. If the file disappeared after the
/// card was hung (moved, pruned by another tool, or recycled elsewhere) there is nothing to
/// recycle: the card must still leave the line instead of surfacing an error and staying stuck.
/// Non-recycle decisions pass through unchanged.
pub fn resolve_action(action: TakeDownAction, path: &Path) -> TakeDownAction {
    match action {
        TakeDownAction::Recycle if !path.is_file() => TakeDownAction::OffLineOnly,
        action => action,
    }
}

/// Moves `path` into the Recycle Bin through `IFileOperation` (undoable, no confirmation).
pub fn recycle(path: &Path) -> anyhow::Result<()> {
    if !path.is_absolute() {
        bail!("refusing to recycle a relative path: {}", path.display());
    }
    if !path.exists() {
        bail!("cannot recycle a missing file: {}", path.display());
    }
    let wide = wide_path(path);
    let _com = ComInit::new()?;

    // SAFETY: COM is initialised on this thread, `wide` is NUL-terminated and outlives every call,
    // and every returned interface is released by its own `Drop`.
    unsafe {
        let operation: IFileOperation = CoCreateInstance(&FileOperation, None, CLSCTX_ALL)
            .context("CoCreateInstance(FileOperation) failed")?;
        operation
            .SetOperationFlags(FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOFX_RECYCLEONDELETE)
            .context("IFileOperation::SetOperationFlags failed")?;
        let item: IShellItem = SHCreateItemFromParsingName(PCWSTR::from_raw(wide.as_ptr()), None)
            .with_context(|| format!("cannot resolve {}", path.display()))?;
        operation
            .DeleteItem(&item, None)
            .context("IFileOperation::DeleteItem failed")?;
        operation
            .PerformOperations()
            .context("IFileOperation::PerformOperations failed")?;
        if operation
            .GetAnyOperationsAborted()
            .context("IFileOperation::GetAnyOperationsAborted failed")?
            .as_bool()
        {
            bail!("the recycle operation was aborted: {}", path.display());
        }
    }

    log::info!(
        "trash: recycled {} into the Recycle Bin (FOF_ALLOWUNDO)",
        path.display()
    );
    Ok(())
}

/// Opens `path` with its default application (ShellExecuteW "open").
pub fn open_path(path: &Path) -> anyhow::Result<()> {
    if !path.is_absolute() {
        bail!("refusing to open a relative path: {}", path.display());
    }
    if !path.exists() {
        bail!("cannot open a missing file: {}", path.display());
    }
    let wide = wide_path(path);
    // SAFETY: `wide` is NUL-terminated for the duration of the call; no shell-owned memory is
    // stored by this function.
    let result = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            PCWSTR::from_raw(wide.as_ptr()),
            None,
            None,
            SW_SHOWNORMAL,
        )
    };
    // ShellExecuteW documents a return value <= 32 as an error code and > 32 as success.
    let code = result.0 as isize;
    if code <= 32 {
        bail!(
            "ShellExecuteW(open, {}) failed with code {code}",
            path.display()
        );
    }
    log::info!(
        "shell: opened {} with the default app (ShellExecuteW code {code})",
        path.display()
    );
    Ok(())
}

/// NUL-terminated UTF-16 form of `path` for the Windows APIs above.
fn wide_path(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
}

/// RAII COM apartment initialisation for the shell calls above.
///
/// `pub(crate)` because the Task 15 folder picker runs on its own thread and needs the same
/// apartment setup (a new-style `SHBrowseForFolderW` dialog requires an initialized COM apartment).
pub(crate) struct ComInit {
    /// Whether this call actually initialised COM and therefore must call `CoUninitialize`.
    owns: bool,
}

impl ComInit {
    pub(crate) fn new() -> anyhow::Result<Self> {
        let hr: HRESULT = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        if hr == RPC_E_CHANGED_MODE {
            // Another part of this thread already initialised COM with a different model; the
            // shell APIs below can still run, but we must not uninitialise their apartment.
            log::debug!("trash: COM already initialised with another threading model");
            return Ok(Self { owns: false });
        }
        hr.ok().context("CoInitializeEx failed")?;
        Ok(Self { owns: true })
    }
}

impl Drop for ComInit {
    fn drop(&mut self) {
        if self.owns {
            // SAFETY: this guard owns one successful CoInitializeEx call on this thread.
            unsafe { CoUninitialize() };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;

    /// A synthetic absolute path under `%LOCALAPPDATA%` (no file system access).
    fn own_dir() -> PathBuf {
        PathBuf::from(r"C:\Users\Example\AppData\Local\Hangline\Screenshots")
    }

    fn action(kind: ItemKind, path: &str) -> TakeDownAction {
        take_down_action(kind, Path::new(path), &own_dir())
    }

    #[test]
    fn capture_and_clipboard_files_inside_our_directory_are_recycled() {
        assert_eq!(
            action(
                ItemKind::Capture,
                r"C:\Users\Example\AppData\Local\Hangline\Screenshots\shot.png"
            ),
            TakeDownAction::Recycle
        );
        assert_eq!(
            action(
                ItemKind::Clipboard,
                r"C:\Users\Example\AppData\Local\Hangline\Screenshots\Clipboard 2026.png"
            ),
            TakeDownAction::Recycle
        );
        assert_eq!(
            action(
                ItemKind::Capture,
                r"C:\Users\Example\AppData\Local\Hangline\Screenshots\2026\10\shot.png"
            ),
            TakeDownAction::Recycle
        );
    }

    #[test]
    fn path_matching_is_case_insensitive_and_resolves_dot_segments() {
        assert_eq!(
            action(
                ItemKind::Capture,
                r"c:\users\EXAMPLE\appdata\local\hangline\SCREENSHOTS\shot.png"
            ),
            TakeDownAction::Recycle
        );
        assert_eq!(
            action(
                ItemKind::Capture,
                r"C:\Users\Example\AppData\Local\Hangline\Screenshots\.\sub\..\shot.png"
            ),
            TakeDownAction::Recycle
        );
    }

    #[test]
    fn a_dot_dot_segment_cannot_escape_our_directory() {
        assert_eq!(
            action(
                ItemKind::Capture,
                r"C:\Users\Example\AppData\Local\Hangline\Screenshots\..\..\evil.png"
            ),
            TakeDownAction::OffLineOnly
        );
        assert_eq!(
            action(
                ItemKind::Capture,
                r"C:\Users\Example\AppData\Local\Hangline\Screenshots\..\Screenshots2\evil.png"
            ),
            TakeDownAction::OffLineOnly
        );
    }

    #[test]
    fn a_shared_name_prefix_or_the_directory_itself_is_not_inside() {
        assert_eq!(
            action(
                ItemKind::Capture,
                r"C:\Users\Example\AppData\Local\Hangline\Screenshots2\shot.png"
            ),
            TakeDownAction::OffLineOnly
        );
        assert_eq!(
            action(
                ItemKind::Capture,
                r"C:\Users\Example\AppData\Local\Hangline\Screenshots"
            ),
            TakeDownAction::OffLineOnly
        );
    }

    #[test]
    fn inbox_items_are_offline_only_even_inside_our_directory() {
        assert_eq!(
            action(
                ItemKind::WatchedFolder,
                r"C:\Users\Example\AppData\Local\Hangline\Screenshots\from-watched.png"
            ),
            TakeDownAction::OffLineOnly,
            "watched-folder files belong to the user's folder"
        );
    }

    #[test]
    fn system_screenshots_and_recordings_are_offline_only() {
        assert_eq!(
            action(
                ItemKind::SystemScreenshot,
                r"C:\Users\Example\Pictures\Screenshots\shot.png"
            ),
            TakeDownAction::OffLineOnly
        );
        assert_eq!(
            action(
                ItemKind::Recording,
                r"C:\Users\Example\Videos\Captures\clip.mp4"
            ),
            TakeDownAction::OffLineOnly
        );
    }

    #[test]
    fn a_missing_file_is_taken_off_the_line_instead_of_recycled() {
        let missing = std::env::temp_dir().join(format!(
            "hangline-task12-missing-{}-{}.png",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock after the epoch")
                .as_nanos()
        ));
        assert!(!missing.exists(), "the test path must not exist");

        assert_eq!(
            resolve_action(TakeDownAction::Recycle, &missing),
            TakeDownAction::OffLineOnly,
            "a missing file must not be sent to the recycler"
        );
        assert_eq!(
            resolve_action(TakeDownAction::OffLineOnly, &missing),
            TakeDownAction::OffLineOnly,
            "an offline-only decision is unchanged"
        );
    }

    #[test]
    fn recycle_rejects_a_relative_path_before_touching_the_shell() {
        assert!(
            recycle(Path::new("shot.png")).is_err(),
            "only absolute paths may be recycled"
        );
    }
}
