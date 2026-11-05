//! Native folder picker for the tray's "Add Folder…" action (Task 15).
//!
//! Windows' classic `SHBrowseForFolderW` shell dialog is used instead of `IFileOpenDialog`: it is a
//! single shell call, needs no COM interface plumbing beyond the apartment initialization, and the
//! design spec explicitly allows either API. No new dependency is needed — `SHBrowseForFolderW` is
//! already available through the `windows` crate features in `Cargo.toml`.
//!
//! The dialog is modal to the calling thread only, so callers must run [`pick_folder`] on a
//! background thread (the tray menu callback runs on Tauri's main thread and must never block the
//! event loop). Cancelling the dialog returns `None` (the task contract: "cancel = None").

use std::path::PathBuf;

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::{
    SHBrowseForFolderW, SHGetPathFromIDListW, BROWSEINFOW, BIF_NEWDIALOGSTYLE, BIF_RETURNONLYFSDIRS,
};

use crate::trash::ComInit;

/// Shows the system folder picker and returns the selected absolute directory.
///
/// `owner` is the dialog's owner window (`None` = no owner, the dialog still appears on top of the
/// picker thread's desktop). `title` is the localized picker caption from [`crate::i18n`].
/// Returns `None` when the user cancels (or when the shell cannot resolve a path), logging the
/// reason instead of surfacing an error — a cancelled picker is not an error condition.
pub fn pick_folder(owner: Option<HWND>, title: &str) -> Option<PathBuf> {
    // New-style browse dialog: requires an initialized COM apartment on this thread.
    let _com: Option<ComInit> = match ComInit::new() {
        Ok(com) => Some(com),
        Err(error) => {
            log::warn!("folder picker: cannot initialize COM ({error:#}); opening the dialog anyway");
            None
        }
    };

    let title_wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    let mut display_name = [0u16; 260];
    let browse = BROWSEINFOW {
        hwndOwner: owner.unwrap_or_default(),
        pidlRoot: std::ptr::null_mut(),
        pszDisplayName: PWSTR(display_name.as_mut_ptr()),
        lpszTitle: PCWSTR(title_wide.as_ptr()),
        // Only directories; the new style gives the resizable explorer-like dialog.
        ulFlags: BIF_RETURNONLYFSDIRS | BIF_NEWDIALOGSTYLE,
        lpfn: None,
        lParam: LPARAM(0),
        iImage: 0,
    };

    // SAFETY: `browse` and the buffers it points at outlive the modal call; the returned PIDL is
    // shell-allocated and freed with CoTaskMemFree below.
    let pidl = unsafe { SHBrowseForFolderW(&browse) };
    if pidl.is_null() {
        log::info!("folder picker: cancelled by the user");
        return None;
    }

    let mut path_buffer = [0u16; 260];
    // SAFETY: `pidl` is a valid PIDL returned above and `path_buffer` is writable for its length.
    let resolved = unsafe { SHGetPathFromIDListW(pidl, &mut path_buffer) };
    // SAFETY: the shell allocated the PIDL; every path returns it exactly once.
    unsafe { CoTaskMemFree(Some(pidl as *const core::ffi::c_void)) };

    if !resolved.as_bool() {
        log::warn!("folder picker: SHGetPathFromIDListW could not resolve the selection");
        return None;
    }
    let len = path_buffer
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(path_buffer.len());
    let path = String::from_utf16_lossy(&path_buffer[..len]);
    if path.trim().is_empty() {
        log::info!("folder picker: the selected PIDL has no file-system path");
        return None;
    }
    log::info!("folder picker: selected {path}");
    Some(PathBuf::from(path))
}
