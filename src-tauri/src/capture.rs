//! GDI screen capture (P0-E5), the screenshot-directory/PNG save helpers, and the interactive
//! freeze-frame capture overlay (Task 16, design spec §3.2 A).
//!
//! # Why hand-written GDI
//!
//! The app needs the whole virtual desktop in one image (all monitors, physical pixels) and a
//! region crop for the screenshot overlay. `windows` (GDI) is already a dependency, so the
//! pipeline is `GetDC(NULL)` → compatible DC/bitmap → `BitBlt(SRCCOPY | CAPTUREBLT)` over
//! `SM_XVIRTUALSCREEN`/`SM_YVIRTUALSCREEN`/`SM_CXVIRTUALSCREEN`/`SM_CYVIRTUALSCREEN` →
//! `GetDIBits` into a **top-down 32-bit** buffer → BGRA→RGBA.
//!
//! # Interactive capture flow (Task 16)
//!
//! [`request_capture`] hides the line first (the panel is layered and `CAPTUREBLT` would freeze it
//! into the shot), grabs the virtual screen, saves it as `%LOCALAPPDATA%\Hangline\capture-tmp.png`
//! (inside the asset-protocol scope) and shows a borderless, always-on-top, focusable `capture`
//! webview exactly covering the virtual screen in **physical** pixels. The frontend renders the
//! frozen frame with `convertFileSrc`, lets the user drag a region, and calls [`finish_capture`]
//! with the selection in virtual-desktop physical pixels (CSS rect × `devicePixelRatio` + the
//! virtual origin; never assuming 100% DPI). [`cancel_capture`] (Esc/right click) and window
//! destruction clean up without creating anything.
//!
//! The save order is the Task 8 dedupe contract (spec §3.2 rule 2), implemented in
//! [`save_capture_image`]: encode to `<final>.png.tmp` → hash the exact bytes → `Deduper::remember`
//! → rename to the final suffixed name → `AppState::add_item` + `emit_state` + new-item reveal.
//!
//! # Mixed-DPI correctness
//!
//! The process must be per-monitor DPI aware (Tauri/tao sets PMv2) or Windows virtualizes the
//! monitor rectangles and reports the 150% monitor as 96 DPI, which corrupts mixed-DPI
//! geometry. The rectangles used here are physical pixels, the same space as
//! [`crate::geometry::monitors`].
//!
//! # Protected content
//!
//! DRM/protected windows may come back black in a GDI capture. That is an OS restriction, not
//! an error: the image is still returned (documented in `docs/experiments/P0-E5.md`). The
//! lock screen's secondary monitors may also be black because Windows only paints the lock
//! UI on the primary monitor.
//!
//! # Side effects and threading
//!
//! Everything here is synchronous and thread-agnostic (GDI screen DCs are per-thread but not
//! tied to the UI thread). All HDC/HBITMAP/GDI selections are released by RAII guards, including
//! on error paths.

use std::fs;
use std::io::{BufWriter, Write};
use std::mem::size_of;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context};
use image::codecs::png::PngEncoder;
use image::{ExtendedColorType, ImageEncoder, RgbaImage};
use serde::Serialize;
use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder,
};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, SYSTEMTIME};
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits,
    ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CAPTUREBLT, DIB_RGB_COLORS,
    HBITMAP, HDC, HGDIOBJ, RGBQUAD, SRCCOPY,
};
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, IsWindowVisible, MessageBoxW, MB_ICONERROR, MB_OK, MB_SETFOREGROUND,
    MB_TOPMOST, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

use crate::app_state::{self, AppState};
use crate::dedupe::{self, Deduper};
use crate::fly;
use crate::geometry::PhysicalRect;
use crate::item::{Item, ItemKind};
use crate::panel;
use crate::reveal;
use crate::settings::Theme;

/// Directory (relative to `%LOCALAPPDATA%`) that holds Hangline's own captures.
///
/// This is our own data directory, **not** Tauri's `app_local_data_dir()`
/// (`%LOCALAPPDATA%\app.hangline.Hangline`), so it stays inside the asset-protocol scope
/// `$LOCALDATA/Hangline/**` configured in `tauri.conf.json`.
pub const SCREENSHOTS_SUBDIR: &str = "Screenshots";

/// Returns the virtual desktop rectangle in physical pixels.
///
/// `SM_*VIRTUALSCREEN` already accounts for monitors with negative origins (left/above the
/// primary). Per-monitor DPI awareness is still required for the monitor rectangles/scale
/// factors to be physical (see the module docs).
pub fn virtual_screen_rect() -> PhysicalRect {
    // SAFETY: GetSystemMetrics has no pointer arguments and cannot fail in a way that
    // invalidates the returned values.
    unsafe {
        PhysicalRect {
            x: GetSystemMetrics(SM_XVIRTUALSCREEN),
            y: GetSystemMetrics(SM_YVIRTUALSCREEN),
            w: GetSystemMetrics(SM_CXVIRTUALSCREEN),
            h: GetSystemMetrics(SM_CYVIRTUALSCREEN),
        }
    }
}

/// Grabs the whole virtual desktop (all monitors) into one RGBA image.
///
/// The image origin is the virtual-screen origin, which can be negative:
/// `image_pixel = physical_point - virtual_screen_rect().origin`. Use [`crop`] with
/// image-relative coordinates, or subtract the origin first.
///
/// Protected/DRM windows may be black; that is a system restriction and not an error.
///
/// The interactive capture uses [`grab_virtual_screen_in`] with its single sampled rectangle; this
/// wrapper is the simple public form and the E5 runtime-evidence helper.
#[allow(dead_code)] // E5 runtime evidence + the documented public API; capture.rs uses the _in form.
pub fn grab_virtual_screen() -> anyhow::Result<RgbaImage> {
    grab_virtual_screen_in(virtual_screen_rect())
}

/// [`grab_virtual_screen`] for one explicitly sampled virtual-screen rectangle.
///
/// The capture request reads [`virtual_screen_rect`] **once** and uses the same rectangle for the
/// grab, the overlay window and the crop transform, so a display change between those steps cannot
/// mix two coordinate spaces (review fix).
pub fn grab_virtual_screen_in(screen_rect: PhysicalRect) -> anyhow::Result<RgbaImage> {
    if screen_rect.w <= 0 || screen_rect.h <= 0 {
        bail!(
            "the virtual screen has an empty size: {}x{}",
            screen_rect.w,
            screen_rect.h
        );
    }
    let buffer_len = (screen_rect.w as usize)
        .checked_mul(screen_rect.h as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| {
            anyhow!(
                "the virtual screen is too large to capture: {}x{}",
                screen_rect.w,
                screen_rect.h
            )
        })?;

    // Declare the guards in acquisition order; they are released in reverse order at the end
    // (bitmap → memory DC → screen DC), also when any `?` below returns early.
    let screen_dc = ScreenDc::acquire()?;
    let memory_dc = MemoryDc::create(screen_dc.0)?;
    let bitmap = GdiBitmap::compatible(screen_dc.0, screen_rect.w, screen_rect.h)?;
    let selection = SelectGuard::select(memory_dc.0, HGDIOBJ(bitmap.0.0))?;

    // CAPTUREBLT also captures layered windows (the panel is layered); SRCCOPY is the pixel
    // copy. The source coordinates are the virtual-screen origin.
    unsafe {
        BitBlt(
            memory_dc.0,
            0,
            0,
            screen_rect.w,
            screen_rect.h,
            Some(screen_dc.0),
            screen_rect.x,
            screen_rect.y,
            SRCCOPY | CAPTUREBLT,
        )
    }
    .context("BitBlt of the virtual screen failed")?;

    // GetDIBits requires the bitmap not to be selected into a device context.
    drop(selection);

    // Top-down 32-bit DIB: negative height makes the first buffer row the top screen row.
    let mut info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: screen_rect.w,
            biHeight: -screen_rect.h,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        bmiColors: [RGBQUAD::default()],
    };
    let mut bgra = vec![0u8; buffer_len];
    let lines = unsafe {
        GetDIBits(
            screen_dc.0,
            bitmap.0,
            0,
            screen_rect.h as u32,
            Some(bgra.as_mut_ptr().cast()),
            &mut info,
            DIB_RGB_COLORS,
        )
    };
    if lines != screen_rect.h {
        bail!(
            "GetDIBits copied {lines} of {} scan lines: {}",
            screen_rect.h,
            std::io::Error::last_os_error()
        );
    }

    rgba_from_bgra(bgra, screen_rect.w as u32, screen_rect.h as u32)
}

/// Crops `image` to `rect`, clamping it to the image bounds.
///
/// `rect` is in image-relative pixels (for a virtual-screen grab: global physical coordinates
/// minus [`virtual_screen_rect`]'s origin). A rectangle that does not overlap the image, or a
/// degenerate one (zero/negative size), yields a 0x0 image.
pub fn crop(image: &RgbaImage, rect: PhysicalRect) -> RgbaImage {
    let image_w = i32::try_from(image.width()).unwrap_or(i32::MAX);
    let image_h = i32::try_from(image.height()).unwrap_or(i32::MAX);
    let x0 = rect.x.max(0);
    let y0 = rect.y.max(0);
    let x1 = rect.x.saturating_add(rect.w).min(image_w);
    let y1 = rect.y.saturating_add(rect.h).min(image_h);
    if x1 <= x0 || y1 <= y0 {
        return RgbaImage::new(0, 0);
    }
    image::imageops::crop_imm(image, x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32)
        .to_image()
}

/// Crops a frozen virtual screen to a rectangle in **virtual-desktop** physical coordinates.
///
/// The capture overlay works in the same physical space as the cursor (`GetCursorPos`/WebView2
/// client pixels), while [`grab_virtual_screen`]'s image origin is the virtual-screen origin —
/// negative when a monitor sits left of or above the primary. `crop` alone assumes image-relative
/// coordinates, so this is the conversion every own capture needs (design spec §3.2 A / §3.9).
/// The result is clamped to the image like [`crop`].
///
/// The interactive capture compares against the virtual screen recorded at freeze time through
/// [`crop_virtual_with_origin`] (a display change while the overlay is open must not shift the
/// crop); this wrapper is the simple public form and the negative-origin contract test.
#[allow(dead_code)] // The session keeps its freeze-time screen rect; this is the documented API.
pub fn crop_virtual(image: &RgbaImage, rect: PhysicalRect) -> RgbaImage {
    crop_virtual_with_origin(image, rect, virtual_screen_rect())
}

/// [`crop_virtual`] with an explicit captured-screen rectangle, so the subtraction is unit-testable
/// without depending on the desktop the test happens to run on.
pub fn crop_virtual_with_origin(
    image: &RgbaImage,
    rect: PhysicalRect,
    screen: PhysicalRect,
) -> RgbaImage {
    crop(
        image,
        PhysicalRect {
            x: rect.x.saturating_sub(screen.x),
            y: rect.y.saturating_sub(screen.y),
            w: rect.w,
            h: rect.h,
        },
    )
}

/// Returns whether an overlay selection has a positive physical size.
///
/// A pointerdown/up without a drag (or an inverted/broken rectangle) must never become a 0x0
/// screenshot; the capture is rejected before anything is encoded.
pub fn is_valid_selection(rect: PhysicalRect) -> bool {
    rect.w > 0 && rect.h > 0
}

/// Returns `%LOCALAPPDATA%\Hangline` (settings/state/screenshots and the capture temp frame).
///
/// Deliberately independent of Tauri: the path must match the asset-protocol scope
/// `$LOCALDATA/Hangline/**` configured in `tauri.conf.json`, which is what lets the capture overlay
/// render the frozen frame with `convertFileSrc`. Missing `%LOCALAPPDATA%` is an error rather than a
/// silent fallback to a directory outside the scope.
fn hangline_dir() -> anyhow::Result<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            anyhow!("LOCALAPPDATA is not set; cannot locate %LOCALAPPDATA%\\Hangline")
        })?;
    Ok(PathBuf::from(base).join("Hangline"))
}

/// Returns `%LOCALAPPDATA%\Hangline\Screenshots`, creating it when missing.
///
/// This is Hangline's own capture directory, **not** Tauri's `app_local_data_dir()`
/// (`%LOCALAPPDATA%\app.hangline.Hangline`), and it is one of the watched folders (Task 8): the
/// own-capture claim in [`save_capture_image`] keeps its watcher from hanging the same file twice.
pub fn screenshots_dir() -> anyhow::Result<PathBuf> {
    let dir = hangline_dir()?.join(SCREENSHOTS_SUBDIR);
    fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create screenshot dir {}", dir.display()))?;
    Ok(dir)
}

/// File name of the frozen frame the capture overlay displays (`%LOCALAPPDATA%\Hangline`).
pub const CAPTURE_TMP_FILE: &str = "capture-tmp.png";

/// Returns `%LOCALAPPDATA%\Hangline\capture-tmp.png` (inside the asset scope, see [`hangline_dir`]).
///
/// The directory is not created here: Hangline's data directory already exists (settings/state),
/// and a capture cannot start before the app is running.
pub fn capture_tmp_path() -> anyhow::Result<PathBuf> {
    Ok(hangline_dir()?.join(CAPTURE_TMP_FILE))
}

/// Writes the frozen virtual screen for the overlay (atomic replace of `capture-tmp.png`).
pub fn save_frozen_frame(image: &RgbaImage) -> anyhow::Result<PathBuf> {
    let path = capture_tmp_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    save_png_atomic(&path, image)?;
    Ok(path)
}

/// Deletes a frozen frame left behind by a crash/kill (called once from the Tauri setup).
///
/// The overlay always deletes its own temp file on finish/cancel; this is the crash residue guard
/// so a stale full-desktop PNG never survives in the user's data directory across restarts.
pub fn remove_stale_frozen_frame() {
    let result = capture_tmp_path().and_then(|path| {
        remove_file_if_exists(&path)
            .with_context(|| format!("failed to remove the stale frozen frame {}", path.display()))
    });
    match result {
        Ok(true) => log::info!("capture: removed a stale capture-tmp.png from a previous run"),
        Ok(false) => {}
        Err(error) => log::warn!("capture: cannot clean the stale frozen frame: {error:#}"),
    }
}

/// Removes `path` when it exists; returns whether a file was removed (missing files are not errors).
fn remove_file_if_exists(path: &Path) -> std::io::Result<bool> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

/// Reports a capture failure to the user with the spec §3.11 native message box (never silent).
///
/// Called only from the capture worker thread after cleanup; a message box works from any thread
/// and blocks that (non-UI) worker until the user acknowledges it.
fn show_capture_error(message: &str) {
    let text: Vec<u16> = format!("Hangline: {message}")
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let title: Vec<u16> = "Hangline".encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: both strings are NUL-terminated and outlive the call; the owner-less box is the
    // expected UX for an app with no focused window (the overlay may already be destroyed).
    unsafe {
        MessageBoxW(
            None,
            PCWSTR(text.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_OK | MB_ICONERROR | MB_SETFOREGROUND | MB_TOPMOST,
        );
    }
}

/// Saves `image` into the Hangline screenshot directory and returns the written path.
///
/// Own captures go through [`save_capture_image`] instead (same directory, plus the dedupe claim
/// and the same-second suffix); this simple writer remains the E5 runtime-evidence helper and the
/// shared implementation behind [`save_png_to`].
#[allow(dead_code)] // Used by the ignored E5 runtime-evidence test and by save_png_to.
pub fn save_png(image: &RgbaImage) -> anyhow::Result<PathBuf> {
    let dir = screenshots_dir()?;
    save_png_to(&dir, image)
}

/// Saves `image` as a timestamped PNG in `dir` with an atomic `.tmp` + rename write.
///
/// The name is local time to the second (`Hangline yyyy-MM-dd HH.mm.ss.png`). This helper is used
/// for one-off saves/evidence; the interactive capture uses [`save_capture_image`], which shares
/// the same naming rules plus the same-second collision suffix.
pub fn save_png_to(dir: &Path, image: &RgbaImage) -> anyhow::Result<PathBuf> {
    fs::create_dir_all(dir)
        .with_context(|| format!("failed to create screenshot dir {}", dir.display()))?;
    let filename = png_filename(unsafe { GetLocalTime() });
    let path = dir.join(filename);
    save_png_atomic(&path, image)?;
    Ok(path)
}

/// Writes one PNG atomically: encode+sync to `<name>.png.tmp`, then rename into place.
fn save_png_atomic(path: &Path, image: &RgbaImage) -> anyhow::Result<()> {
    let tmp_path = path.with_extension("png.tmp");
    let written = write_png(&tmp_path, image).and_then(|()| {
        fs::rename(&tmp_path, path)
            .with_context(|| format!("failed to rename {} to {}", tmp_path.display(), path.display()))
    });
    if written.is_err() {
        // Best effort: a stale .tmp must never masquerade as a screenshot.
        let _ = fs::remove_file(&tmp_path);
    }
    written
}

/// Encodes `image` as PNG, flushes it to disk, and closes the file.
fn write_png(path: &Path, image: &RgbaImage) -> anyhow::Result<()> {
    let file = fs::File::create(path)
        .with_context(|| format!("failed to create {}", path.display()))?;
    let mut writer = BufWriter::new(file);
    PngEncoder::new(&mut writer)
        .write_image(
            image.as_raw(),
            image.width(),
            image.height(),
            ExtendedColorType::Rgba8,
        )
        .with_context(|| format!("failed to encode PNG {}", path.display()))?;
    writer
        .flush()
        .with_context(|| format!("failed to flush PNG {}", path.display()))?;
    let file = writer.into_inner().map_err(|err| {
        anyhow!(
            "failed to finish writing PNG {}: {err}",
            path.display()
        )
    })?;
    file.sync_all()
        .with_context(|| format!("failed to sync PNG {}", path.display()))?;
    drop(file);
    Ok(())
}

/// Formats the screenshot file stem in local time: `Hangline yyyy-MM-dd HH.mm.ss`.
fn png_stem(local: SYSTEMTIME) -> String {
    format!(
        "Hangline {:04}-{:02}-{:02} {:02}.{:02}.{:02}",
        local.wYear, local.wMonth, local.wDay, local.wHour, local.wMinute, local.wSecond
    )
}

/// Formats the screenshot file name in local time: `Hangline yyyy-MM-dd HH.mm.ss.png`.
fn png_filename(local: SYSTEMTIME) -> String {
    format!("{}.png", png_stem(local))
}

/// Number of `-N` collision suffixes tried before falling back to a millisecond suffix.
///
/// Shared with the clipboard saver (`watchers::clipboard` delegates here), so both own save paths
/// behave identically on a same-second burst: never silently overwrite an earlier image.
pub const MAX_NAME_SUFFIX: u32 = 99;

/// Picks a free `<stem>.png` path in `dir`, numbering same-second collisions (`-2`, `-3`, ...).
///
/// Design spec 4.6 permits a same-second overwrite, but two captures within one second are
/// realistic and losing the first image is worse than a suffix. If all 99 numbered names are taken
/// the name falls back to a millisecond suffix; the content-hash dedupe remains the durable guard.
pub fn unique_png_path(dir: &Path, stem: &str) -> PathBuf {
    let first = dir.join(format!("{stem}.png"));
    if !first.exists() {
        return first;
    }
    for index in 2..=MAX_NAME_SUFFIX {
        let candidate = dir.join(format!("{stem}-{index}.png"));
        if !candidate.exists() {
            return candidate;
        }
    }
    dir.join(format!("{stem}-{}.png", crate::item::now_ms()))
}

/// One own capture persisted through the Task 8 claim contract (spec §3.2 rule 2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedCapture {
    /// Final path (`Hangline yyyy-MM-dd HH.mm.ss.png`, suffixed on a same-second collision).
    pub path: PathBuf,
    /// BLAKE3 hash of the exact PNG bytes on disk, truncated to 64 bits (the dedupe key).
    pub hash: u64,
    /// Cropped pixel width.
    pub width: u32,
    /// Cropped pixel height.
    pub height: u32,
}

/// The steps [`save_capture_image_observed`] goes through, in the order the Task 8 contract fixes.
///
/// Exposed to the unit tests as an observation seam so "remember before the final name is visible"
/// is asserted by construction instead of inferred from the final state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CaptureSaveStep {
    /// A final name was atomically reserved with `create_new` (empty placeholder file).
    Reserved,
    /// The encoded PNG was written + synced to `<final>.png.tmp`.
    TmpWritten,
    /// The exact temp-file bytes were hashed with BLAKE3-64.
    Hashed,
    /// `Deduper::remember` recorded the pending claim.
    Claimed,
    /// The temp file was renamed over the reservation to the final name.
    Renamed,
}

/// Saves one own capture with the exact claim order required by spec §3.2 rule 2 / Task 8.
///
/// 1. atomically **reserve** a free final name with `create_new` (TOCTOU fix: a clipboard save can
///    never create the same name between an existence check and the rename);
/// 2. encode the crop into `<final>.png.tmp` inside `dir`;
/// 3. hash the **exact temp-file bytes** (`hash_file`) — the rename does not change them, so the
///    own-captures folder watcher computes the same digest after the rename;
/// 4. `Deduper::remember(hash)` — the 5 s pending claim exists before the file content is visible
///    under its final name (the reservation itself is empty and ignored by the watcher), so the
///    watcher's event is suppressed instead of hanging a duplicate;
/// 5. rename the temp file over the reservation (`Hangline yyyy-MM-dd HH.mm.ss.png`, suffixed when
///    another writer already used that second);
/// 6. return path/hash/dimensions for `Item::new` + `AppState::add_item`.
///
/// A failure after the hash was claimed releases it (`forget`); the partially written `.png.tmp`
/// and the empty reservation are removed so neither can masquerade as a screenshot.
pub fn save_capture_image(
    dir: &Path,
    image: &RgbaImage,
    deduper: &Deduper,
) -> anyhow::Result<SavedCapture> {
    save_capture_image_observed(dir, image, deduper, unsafe { GetLocalTime() }, |_, _| {})
}

/// [`save_capture_image`] with an explicit local time (deterministic same-second unit tests).
#[cfg(test)]
fn save_capture_image_at(
    dir: &Path,
    image: &RgbaImage,
    deduper: &Deduper,
    local: SYSTEMTIME,
) -> anyhow::Result<SavedCapture> {
    save_capture_image_observed(dir, image, deduper, local, |_, _| {})
}

/// [`save_capture_image_at`] with a step observer (the unit tests assert the claim order).
fn save_capture_image_observed<F>(
    dir: &Path,
    image: &RgbaImage,
    deduper: &Deduper,
    local: SYSTEMTIME,
    mut observe: F,
) -> anyhow::Result<SavedCapture>
where
    F: FnMut(CaptureSaveStep, &Path),
{
    fs::create_dir_all(dir)
        .with_context(|| format!("failed to create screenshot dir {}", dir.display()))?;
    let path = reserve_capture_path(dir, &png_stem(local))?;
    observe(CaptureSaveStep::Reserved, &path);
    let tmp_path = path.with_extension("png.tmp");
    let mut claimed: Option<u64> = None;
    let saved = (|| -> anyhow::Result<SavedCapture> {
        write_png(&tmp_path, image)?;
        observe(CaptureSaveStep::TmpWritten, &path);
        let hash = dedupe::hash_file(&tmp_path).with_context(|| {
            format!(
                "failed to hash the captured PNG {} before claiming it",
                tmp_path.display()
            )
        })?;
        observe(CaptureSaveStep::Hashed, &path);
        // The claim must exist before the content is visible under its final name.
        deduper.remember(hash);
        claimed = Some(hash);
        observe(CaptureSaveStep::Claimed, &path);
        fs::rename(&tmp_path, &path)
            .with_context(|| format!("failed to rename {} to {}", tmp_path.display(), path.display()))?;
        observe(CaptureSaveStep::Renamed, &path);
        Ok(SavedCapture {
            path: path.clone(),
            hash,
            width: image.width(),
            height: image.height(),
        })
    })();
    if saved.is_err() {
        let _ = fs::remove_file(&tmp_path);
        // Release the empty reservation: without this a failed save would keep the name blocked.
        let _ = fs::remove_file(&path);
        if let Some(hash) = claimed {
            // The content never became visible on the line; a retry must not be a "duplicate".
            deduper.forget(hash);
        }
    }
    saved
}

/// Atomically reserves a free capture file name by creating it exclusively (`create_new`).
///
/// The previous `Path::exists()` + rename sequence was a TOCTOU race: a clipboard save could create
/// `Hangline <second>.png` after our existence check, and `fs::rename` (which replaces an existing
/// destination on Windows) would then silently destroy it. `create_new` makes exactly one writer
/// win the name; the empty reservation is replaced by the finished PNG through the same rename.
fn reserve_capture_path(dir: &Path, stem: &str) -> anyhow::Result<PathBuf> {
    for index in 0..=MAX_NAME_SUFFIX {
        let name = if index == 0 {
            format!("{stem}.png")
        } else {
            format!("{stem}-{}.png", index + 1)
        };
        if let Some(path) = try_reserve(dir, &name)? {
            return Ok(path);
        }
    }
    // Every numbered name is taken: fall back to a millisecond prefix, still reserving exclusively.
    let base = format!("{stem}-{}", crate::item::now_ms());
    for suffix in 0..MAX_NAME_SUFFIX {
        let name = if suffix == 0 {
            format!("{base}.png")
        } else {
            format!("{base}-{suffix}.png")
        };
        if let Some(path) = try_reserve(dir, &name)? {
            return Ok(path);
        }
    }
    bail!(
        "capture: no free screenshot file name for {stem:?} after {} attempts",
        MAX_NAME_SUFFIX
    )
}

/// Tries to atomically create `dir/name`; `Ok(None)` when the name is already taken.
fn try_reserve(dir: &Path, name: &str) -> anyhow::Result<Option<PathBuf>> {
    let path = dir.join(name);
    match fs::OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(_) => Ok(Some(path)),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(None),
        Err(error) => {
            Err(error).with_context(|| format!("failed to reserve {}", path.display()))
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Interactive capture: reentry guard, frozen-frame session, overlay window and finish/cancel
// ---------------------------------------------------------------------------------------------

/// Label of the freeze-frame overlay window (`index.html?window=capture`).
pub const OVERLAY_LABEL: &str = "capture";

/// How long the freeze waits for the panel to disappear before the `BitBlt`.
///
/// The panel disappears synchronously (`ShowWindow(SW_HIDE)`), but the layered window can still be
/// composited into a `CAPTUREBLT` grab for a frame; a bounded wait keeps the freeze responsive.
const PANEL_HIDE_TIMEOUT: Duration = Duration::from_millis(240);

/// Poll interval while waiting for the panel's `IsWindowVisible` to become false.
const PANEL_HIDE_POLL: Duration = Duration::from_millis(15);

/// How long the overlay frontend's ready handshake waits for the frozen frame to be attached.
///
/// The page can load while the request is still finishing (it is shown only after the attach, but
/// the webview navigation starts with the hidden window); waiting here instead of returning an
/// error prevents the frontend from being left with a black overlay and a dropped `capture-ready`.
const OVERLAY_ATTACH_WAIT: Duration = Duration::from_secs(3);

/// Poll interval of the overlay attach wait.
const OVERLAY_ATTACH_POLL: Duration = Duration::from_millis(20);

/// Extra settle time after `IsWindowVisible` is false, letting the compositor drop the layer.
const PANEL_HIDE_SETTLE: Duration = Duration::from_millis(30);

/// Everything one interactive capture owns from "freeze" until finish/cancel.
#[derive(Debug)]
struct CaptureSession {
    /// `%LOCALAPPDATA%\Hangline\capture-tmp.png` displayed by the overlay.
    temp_path: PathBuf,
    /// The virtual-screen rectangle captured into `image` (physical pixels, negative origin kept).
    virtual_rect: PhysicalRect,
    /// The frozen frame itself; the selection crop is taken from these exact pixels.
    image: RgbaImage,
    /// Scale factor of the overlay window (physical px per CSS px): logs + `capture-ready` payload.
    scale: f64,
}

impl CaptureSession {
    /// The serialized `capture-ready` payload (wire shape locked by a unit test).
    fn ready_payload(&self) -> CaptureReady {
        CaptureReady {
            temp_path: self.temp_path.to_string_lossy().into_owned(),
            virtual_rect: self.virtual_rect,
            scale: self.scale,
            width: self.image.width(),
            height: self.image.height(),
        }
    }
}

/// Payload of the Rust → capture-overlay `capture-ready` event (Task 16).
///
/// Emitted only after the overlay frontend calls `report_capture_ready`, so the listener is
/// guaranteed to exist; the frontend renders the frozen frame with `convertFileSrc(tempPath)`.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureReady {
    /// Absolute path of the frozen frame (`%LOCALAPPDATA%\Hangline\capture-tmp.png`).
    pub temp_path: String,
    /// Virtual-screen rectangle of the frozen frame, physical pixels (may start negative).
    pub virtual_rect: PhysicalRect,
    /// Overlay window scale factor (informational; the frontend uses `window.devicePixelRatio`).
    pub scale: f64,
    /// Frozen-frame width in physical pixels.
    pub width: u32,
    /// Frozen-frame height in physical pixels.
    pub height: u32,
}

/// Phases of the single interactive capture managed by [`CaptureCoordinator`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CapturePhase {
    /// No capture in flight; the next `request_capture` may start.
    Idle,
    /// The line is being suspended / the screen grabbed / the frozen frame saved.
    Freezing,
    /// The overlay window is up and waiting for a selection (or cancel).
    Overlay,
    /// Finish/cancel owns the session and is cleaning up; new requests are still rejected.
    Cleaning,
}

/// Ownership token returned by [`CaptureCoordinator::try_begin`].
///
/// The async request path and its error/cleanup paths are owner-checked: a stray cancel or the
/// failure branch of an older request can never clear the phase/session of a newer capture (the
/// review found exactly this race between `take_for_cancel` during `Freezing` and `release`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CaptureLease {
    generation: u64,
}

/// Mutable state behind [`CaptureCoordinator`].
#[derive(Debug)]
struct CaptureSlot {
    phase: CapturePhase,
    /// Incremented on every accepted request; stale owners compare against it.
    generation: u64,
    /// Set from the moment the overlay is attached until finish/cancel takes it exactly once.
    session: Option<CaptureSession>,
}

impl Default for CaptureSlot {
    fn default() -> Self {
        Self {
            phase: CapturePhase::Idle,
            generation: 0,
            session: None,
        }
    }
}

/// Reentry guard plus the one live capture session; managed by Tauri (Task 16).
///
/// A second `Ctrl+Alt+S` while the overlay is open must be a logged no-op, not a second freeze and
/// window; and the frozen frame/temp file must be taken (and cleaned) exactly once even when finish,
/// cancel and a window-destroyed event race each other. The phase machine + generation ownership are
/// unit-tested, including the interleaving where an old request's cleanup must not wipe a new one.
#[derive(Debug, Default)]
pub struct CaptureCoordinator {
    inner: Mutex<CaptureSlot>,
}

impl CaptureCoordinator {
    fn lock(&self) -> MutexGuard<'_, CaptureSlot> {
        // A panic elsewhere must not disable the capture hotkey forever.
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Starts a freeze when idle; `None` means another capture is already in flight.
    fn try_begin(&self) -> Option<CaptureLease> {
        let mut slot = self.lock();
        if slot.phase != CapturePhase::Idle {
            return None;
        }
        slot.generation = slot.generation.wrapping_add(1);
        slot.phase = CapturePhase::Freezing;
        Some(CaptureLease {
            generation: slot.generation,
        })
    }

    /// Registers the frozen frame for the overlay; only the owning `Freezing` request may attach.
    fn attach_overlay(&self, lease: CaptureLease, session: CaptureSession) -> bool {
        let mut slot = self.lock();
        if slot.phase != CapturePhase::Freezing || slot.generation != lease.generation {
            return false;
        }
        slot.session = Some(session);
        slot.phase = CapturePhase::Overlay;
        true
    }

    /// Hands the live overlay session to finish, returning it with its generation.
    ///
    /// `None` when the overlay is not open. If the phase says `Overlay` but the session is missing
    /// (an invariant break), the phase is restored to `Idle` instead of blocking captures forever.
    fn take_for_finish(&self) -> Option<(CaptureSession, u64)> {
        let mut slot = self.lock();
        if slot.phase != CapturePhase::Overlay {
            return None;
        }
        match slot.session.take() {
            Some(session) => {
                slot.phase = CapturePhase::Cleaning;
                Some((session, slot.generation))
            }
            None => {
                log::error!(
                    "capture: the overlay phase had no session; resetting the capture coordinator"
                );
                slot.phase = CapturePhase::Idle;
                None
            }
        }
    }

    /// Cancels the live overlay (frontend Esc/right click or the window-destroyed safety net).
    ///
    /// Only a real overlay can be cancelled: a stray cancel during `Freezing` is a logged no-op, so
    /// it can neither destroy an in-flight request's window nor release its guard.
    fn take_for_cancel(&self) -> Option<(CaptureSession, u64)> {
        let mut slot = self.lock();
        if slot.phase != CapturePhase::Overlay {
            return None;
        }
        let session = slot.session.take()?;
        slot.phase = CapturePhase::Cleaning;
        Some((session, slot.generation))
    }

    /// Aborts the request identified by `lease` (error path); stale owners are ignored.
    ///
    /// Returns the attached session when the request had already reached `Overlay`, so the caller
    /// can delete its frozen frame with the rest of the cleanup.
    fn abort_owned(&self, lease: CaptureLease) -> Option<CaptureSession> {
        let mut slot = self.lock();
        if slot.generation != lease.generation || slot.phase == CapturePhase::Idle {
            return None;
        }
        let session = slot.session.take();
        slot.phase = CapturePhase::Idle;
        session
    }

    /// Releases the `Cleaning` phase after finish/cancel cleanup; a stale generation is ignored.
    fn release_owned(&self, generation: u64) {
        let mut slot = self.lock();
        if slot.generation == generation && slot.phase == CapturePhase::Cleaning {
            slot.phase = CapturePhase::Idle;
        }
    }

    /// The payload to emit again when the overlay frontend reports that it is listening.
    fn ready_payload(&self) -> Option<CaptureReady> {
        let slot = self.lock();
        if slot.phase != CapturePhase::Overlay {
            return None;
        }
        slot.session.as_ref().map(CaptureSession::ready_payload)
    }

    /// Current phase (unit-test assertions).
    #[cfg(test)]
    fn phase(&self) -> CapturePhase {
        self.lock().phase
    }

    /// Breaks the `Overlay` invariant on purpose (unit test for the defensive reset).
    #[cfg(test)]
    fn force_overlay_without_session(&self) {
        let mut slot = self.lock();
        slot.phase = CapturePhase::Overlay;
        slot.session = None;
    }
}

/// Starts the whole interactive capture: suspend the line, freeze, show the overlay window.
///
/// Order matters (design spec §3.2 A / Task 16 carry-overs, T16 review):
/// 1. reentry guard (`try_begin`) so a hotkey burst cannot start two captures;
/// 2. **suspend the reveal controller** (the only owner of `panel::show`/`hide`) with a capture
///    reason; it hides the line and remembers the pre-capture visibility and pending tuck deadline;
/// 3. **hide the fly overlay** (P4-6 preflight): the pre-warmed overlay is a screen-wide window and
///    a flight in progress — or its 160 ms handoff fade — would be frozen into the screenshot;
/// 4. wait until the layered panel's `IsWindowVisible` **and** the overlay's are false (direct Win32
///    probes) and **abort** the capture if they stay visible — a tainted freeze (line/overlay baked
///    into the screenshot) is worse than a failed request;
/// 5. read the virtual-screen rectangle **once**, GDI-grab it, save it as `capture-tmp.png`;
/// 6. create the borderless, always-on-top, focusable overlay covering that exact rectangle in
///    physical pixels (negative origins included), attach the session (owner-checked) and show it.
///
/// Any failure after the suspension resumes the controller, releases the guard, removes the partial
/// temp file/window and reports the failure; it never leaves the line hidden or a window up.
pub fn request_capture(app: &AppHandle) -> anyhow::Result<()> {
    let Some(coordinator) = app.try_state::<CaptureCoordinator>() else {
        bail!("capture: the capture coordinator is not managed");
    };
    let Some(lease) = coordinator.try_begin() else {
        log::info!("capture: a capture is already in progress; ignoring the request");
        return Ok(());
    };

    let mut temp_path: Option<PathBuf> = None;
    let result = request_capture_owned(app, &coordinator, lease, &mut temp_path);
    if let Err(error) = result {
        // Undo only what this request still owns: a stale generation cannot clobber a newer capture.
        let attached = coordinator.abort_owned(lease);
        if let Some(session) = attached {
            cleanup_session(app, &session);
        }
        if let Some(path) = &temp_path {
            let _ = remove_file_if_exists(path);
        }
        destroy_overlay(app);
        // The controller may have hidden the line for this request: always resume it.
        reveal::resume_after_capture(app, false);
        log::error!("capture: request failed: {error:#}");
        show_capture_error(&format!("截图失败：{error}"));
        return Err(error);
    }
    Ok(())
}

/// Body of one owned capture request (kept separate so every failure path shares the cleanup above).
fn request_capture_owned(
    app: &AppHandle,
    coordinator: &CaptureCoordinator,
    lease: CaptureLease,
    temp_path: &mut Option<PathBuf>,
) -> anyhow::Result<()> {
    // The reveal controller hides the line (single owner of panel::show/hide) and remembers what to
    // restore; it blocks until the hide command was applied.
    let was_visible = reveal::suspend_for_capture(app)
        .context("capture: cannot suspend the reveal controller before the freeze")?;
    log::info!(
        "capture: suspended the line before the freeze (CAPTUREBLT would include it, \
         was_visible={was_visible})"
    );
    // P4-6 preflight: the pre-warmed fly overlay is screen-wide and always-on-top. A card that is
    // still flying (or fading out over the landed panel card) must not be baked into the frozen
    // frame, so hide the overlay before the grab and wait for it to leave the screen too.
    let fly_was_visible = fly_overlay_visible(app) == Some(true);
    if fly_was_visible {
        log::info!("capture: hiding the fly overlay before the freeze (a flight is on screen)");
    }
    let fly_hidden = fly::hide_for_capture(app);
    log::info!(
        "capture: fly overlay preflight (was_shown={fly_was_visible}, \
         hide_called={fly_hidden})"
    );
    wait_until_panel_hidden(app)?;
    wait_until_overlay_hidden(app)?;
    log::info!("capture: preflight complete (line and fly overlay are off screen)");

    let screen = virtual_screen_rect();
    let image = grab_virtual_screen_in(screen).context("capture: the GDI virtual-screen grab failed")?;
    let path = save_frozen_frame(&image).context("capture: cannot save the frozen frame")?;
    *temp_path = Some(path.clone());
    log::info!(
        "capture: froze {}x{} px at ({}, {}) into {}",
        image.width(),
        image.height(),
        screen.x,
        screen.y,
        path.display()
    );

    let window = create_overlay_window(app, screen)
        .context("capture: cannot create the overlay window")?;
    let scale = window.scale_factor().unwrap_or(1.0);
    let session = CaptureSession {
        temp_path: path,
        virtual_rect: screen,
        image,
        scale,
    };
    if !coordinator.attach_overlay(lease, session) {
        bail!("capture: the overlay session disappeared before it could be attached");
    }
    window
        .show()
        .context("capture: cannot show the overlay window")?;
    if let Err(error) = window.set_focus() {
        log::warn!("capture: cannot focus the overlay window: {error}");
    }
    log::info!(
        "capture: overlay shown at ({}, {}) {}x{} px (scale {scale:.3})",
        screen.x,
        screen.y,
        screen.w,
        screen.h
    );
    Ok(())
}

/// Runs a capture request on a named worker thread (for event-loop/global-shortcut callers).
///
/// The Tauri global-shortcut plugin dispatches its handler from the event-loop thread, and creating a
/// `WebviewWindow` from the event-loop thread deadlocks (it dispatches to the loop and waits). The
/// hotkey and tray entry points therefore always hand the request to this worker, exactly like the
/// async `request_capture` command does on the tokio pool.
pub fn request_capture_in_background(app: &AppHandle) {
    let app = app.clone();
    let spawned = std::thread::Builder::new()
        .name("hangline-capture".to_string())
        .spawn(move || {
            if let Err(error) = request_capture(&app) {
                log::error!("capture: background request failed: {error:#}");
            }
        });
    if let Err(error) = spawned {
        log::error!("capture: cannot spawn the capture worker thread: {error}");
    }
}

/// Waits (bounded) until the panel's `IsWindowVisible` is false; an error aborts the freeze.
///
/// Uses a direct Win32 probe instead of a Tauri window getter: it is safe from the capture worker
/// thread and cannot block on an event-loop round-trip while the UI thread is busy.
fn wait_until_panel_hidden(app: &AppHandle) -> anyhow::Result<()> {
    wait_until_window_hidden("the line", || panel_window_visible(app), false)
}

/// Waits (bounded) until the pre-warmed fly overlay is off screen (P4-6 preflight).
///
/// The overlay is created hidden at startup and may be missing entirely (a failed pre-warm is
/// logged and the app keeps working), so a missing window counts as hidden for the freeze.
fn wait_until_overlay_hidden(app: &AppHandle) -> anyhow::Result<()> {
    wait_until_window_hidden("the fly overlay", || fly_overlay_visible(app), true)
}

/// Bounded `IsWindowVisible` wait shared by the line and the fly overlay.
///
/// `probe` returns `Some(true)` while the window is visible, `Some(false)` once it is hidden and
/// `None` when the window cannot be read. `missing_is_hidden` decides whether an unreadable window
/// is acceptable (the optional fly overlay) or must abort the capture (the line, which always
/// exists while the app runs). The short settle sleep after a successful wait lets the compositor
/// drop the layered window before the `CAPTUREBLT` grab.
fn wait_until_window_hidden(
    what: &str,
    probe: impl Fn() -> Option<bool>,
    missing_is_hidden: bool,
) -> anyhow::Result<()> {
    let started = Instant::now();
    loop {
        match probe() {
            Some(false) => {
                std::thread::sleep(PANEL_HIDE_SETTLE);
                return Ok(());
            }
            None if missing_is_hidden => {
                std::thread::sleep(PANEL_HIDE_SETTLE);
                return Ok(());
            }
            Some(true) if started.elapsed() < PANEL_HIDE_TIMEOUT => {
                std::thread::sleep(PANEL_HIDE_POLL);
            }
            Some(true) => bail!(
                "{what} is still visible after {} ms; aborting the freeze so it cannot appear \
                 in the screenshot",
                PANEL_HIDE_TIMEOUT.as_millis()
            ),
            None => bail!("cannot read the visibility of {what}; aborting the freeze"),
        }
    }
}

/// Reads the panel's Win32 visibility directly (`IsWindowVisible` is thread-safe); `None` = no window.
fn panel_window_visible(app: &AppHandle) -> Option<bool> {
    let panel = app.get_webview_window(panel::LABEL)?;
    let hwnd: HWND = panel.hwnd().ok()?;
    // SAFETY: IsWindowVisible is a read-only query on a window handle owned by this process.
    Some(unsafe { IsWindowVisible(hwnd).as_bool() })
}

/// Reads the fly overlay's Win32 visibility directly; `None` = the overlay does not exist.
fn fly_overlay_visible(app: &AppHandle) -> Option<bool> {
    let overlay = app.get_webview_window(fly::WINDOW_LABEL)?;
    let hwnd: HWND = overlay.hwnd().ok()?;
    // SAFETY: IsWindowVisible is a read-only query on a window handle owned by this process.
    Some(unsafe { IsWindowVisible(hwnd).as_bool() })
}

/// URL of the capture overlay page (P2-8 theme contract; unit-tested).
pub(crate) fn capture_url(theme: Theme) -> String {
    format!("index.html?window=capture&theme={}", theme.as_str())
}

/// Creates the borderless, always-on-top, focusable overlay covering the whole virtual screen.
///
/// The window is created hidden with the builder (logical arguments would be distorted by mixed
/// DPI) and then positioned/sized in **physical** pixels, so a monitor at `x = -1920` or above the
/// primary is exact. The caller shows it only after the session is attached, so the frontend's
/// ready handshake can never race an empty session.
fn create_overlay_window(app: &AppHandle, screen: PhysicalRect) -> anyhow::Result<WebviewWindow> {
    // P2-8: the overlay is a separate webview with no app store; pass the raw theme setting so it
    // can resolve `system` against its own `prefers-color-scheme` (capture dim/handles/label).
    let theme = app.state::<AppState>().snapshot().settings.theme;
    let mut builder =
        WebviewWindowBuilder::new(app, OVERLAY_LABEL, WebviewUrl::App(capture_url(theme).into()))
            .title("Hangline capture")
            .decorations(false)
            .always_on_top(true)
            .skip_taskbar(true)
            .shadow(false)
            .resizable(false)
            .focused(false)
            .focusable(true)
            .visible(false);
    // Debug-only runtime-evidence hook; no-op in release builds (see `devtools`).
    if let Some(args) = crate::devtools::cdp_browser_args() {
        builder = builder.additional_browser_args(&args);
    }
    let window = builder
        .build()
        .context("failed to build the capture overlay window")?;

    window
        .set_position(PhysicalPosition::new(screen.x, screen.y))
        .context("failed to position the capture overlay")?;
    window
        .set_size(PhysicalSize::new(
            screen.w.max(1) as u32,
            screen.h.max(1) as u32,
        ))
        .context("failed to size the capture overlay")?;

    // A destroyed window (Alt+F4, renderer crash) must not leave the coordinator/session stuck and
    // the line hidden: finish/cancel take the session first, so this is a no-op on the normal paths.
    let app_handle = app.clone();
    window.on_window_event(move |event| {
        if matches!(event, tauri::WindowEvent::Destroyed) {
            abandon_overlay(&app_handle);
        }
    });
    Ok(window)
}

/// Frontend handshake: the overlay page registered its `capture-ready` listener.
///
/// Waits (bounded) for the request to attach its session before emitting: the overlay page can be
/// navigated before the freeze/window hand-off completes, and a lost `capture-ready` would leave a
/// black full-screen overlay. The frontend additionally retries the handshake with backoff.
pub fn report_overlay_ready(app: &AppHandle) -> anyhow::Result<()> {
    let Some(coordinator) = app.try_state::<CaptureCoordinator>() else {
        bail!("capture: the capture coordinator is not managed");
    };
    let started = Instant::now();
    let payload = loop {
        if let Some(payload) = coordinator.ready_payload() {
            break payload;
        }
        if started.elapsed() >= OVERLAY_ATTACH_WAIT {
            bail!(
                "capture: the frozen frame was not attached within {} ms",
                OVERLAY_ATTACH_WAIT.as_millis()
            );
        }
        std::thread::sleep(OVERLAY_ATTACH_POLL);
    };
    let Some(window) = app.get_webview_window(OVERLAY_LABEL) else {
        bail!("capture: the overlay window is not open");
    };
    log::info!(
        "capture: capture-ready ({}x{} px frozen frame, scale {:.3})",
        payload.width,
        payload.height,
        payload.scale
    );
    window
        .emit("capture-ready", payload)
        .context("capture: failed to emit capture-ready")?;
    Ok(())
}

/// Confirms the drag selection (virtual-desktop physical pixels), hangs the capture, then cleans up.
///
/// Completion order (T16 review fix): crop → save through [`save_capture_image`] → `AppState::add_item`
/// → `emit_state` → **cleanup** (destroy the overlay, delete the frozen frame, resume the reveal
/// controller) → the controller decides the new-item peek. Requesting the reveal before cleanup was
/// wrong: the line must never be shown over the frozen screen, and a controller peek that is already
/// running must not be clobbered.
pub fn finish_capture(app: &AppHandle, rect: PhysicalRect) -> anyhow::Result<()> {
    let Some(coordinator) = app.try_state::<CaptureCoordinator>() else {
        bail!("capture: the capture coordinator is not managed");
    };
    let Some((session, generation)) = coordinator.take_for_finish() else {
        log::warn!("capture: finish_capture with no live overlay session (already handled?)");
        bail!("capture: no selection is waiting for confirmation");
    };
    let result = save_finished_capture(app, &session, rect);
    cleanup_session(app, &session);
    coordinator.release_owned(generation);
    // The controller restores the pre-freeze visibility/pending deadline and, for a new capture
    // item, runs the same newItem peek path as the watchers — now that the overlay is gone.
    let new_item_hung = matches!(&result, Ok(true));
    reveal::resume_after_capture(app, new_item_hung);
    match result {
        Ok(_) => {
            log::info!("capture: finished; overlay closed and the line controller resumed");
            Ok(())
        }
        Err(error) => {
            log::error!("capture: finish failed: {error:#}");
            show_capture_error(&format!("截图保存失败：{error}"));
            Err(error)
        }
    }
}

/// Crop + persist + hang one confirmed selection; `Ok(true)` when an item was added.
fn save_finished_capture(
    app: &AppHandle,
    session: &CaptureSession,
    rect: PhysicalRect,
) -> anyhow::Result<bool> {
    if !is_valid_selection(rect) {
        bail!(
            "capture: the selection is empty ({}x{} physical px)",
            rect.w,
            rect.h
        );
    }
    let image = crop_virtual_with_origin(&session.image, rect, session.virtual_rect);
    if image.width() == 0 || image.height() == 0 {
        bail!("capture: the selection {rect:?} does not overlap the frozen screen");
    }

    let dir = screenshots_dir().context("capture: the screenshot folder is unavailable")?;
    let deduper = app.state::<Deduper>();
    let saved = save_capture_image(&dir, &image, &deduper)
        .context("capture: cannot save the selected region")?;

    let mut item = Item::new(
        saved.path.clone(),
        ItemKind::Capture,
        saved.hash,
        saved.width,
        saved.height,
    );
    // P2-7: build the preview from the decoded crop that is already in memory (no second 4K PNG
    // decode) before the first state snapshot; a failure keeps `thumb_path = None`.
    crate::thumbnails::attach_from_image(&mut item, &image);
    let item_id = item.id.clone();
    let state = app.state::<AppState>();
    match state.add_item(item) {
        Ok(_) => {
            // P2-5: remember where this capture came from so the panel can fly the new card in
            // from the selection rect (spec §4.6 C). Recorded before the broadcast below, because
            // the panel's state listener may start the flight as soon as it sees the item.
            state.record_capture_origin(&item_id, rect);
            // The single serialized snapshot+broadcast choke point; its snapshot is the freshest.
            // The reveal decision is delegated to the controller on resume (after cleanup).
            if app_state::emit_state(app).is_none() {
                deduper.forget(saved.hash);
                bail!("capture: the app state is not managed; cannot publish the new item");
            }
            log::info!(
                "capture: hung {} (Capture) {}x{} hash {:016x}",
                saved.path.display(),
                saved.width,
                saved.height,
                saved.hash
            );
            Ok(true)
        }
        Err(error) => {
            // `add_item` mutated memory before the persist failure. The PNG stays in the screenshots
            // folder (its watcher may retry it), so the hash must not stay claimed as a duplicate.
            deduper.forget(saved.hash);
            app_state::emit_state(app);
            Err(error.context("capture: failed to persist the new item"))
        }
    }
}

/// Cancels the overlay (Esc / right click): no file, no item; restores the line.
pub fn cancel_capture(app: &AppHandle) {
    let Some(coordinator) = app.try_state::<CaptureCoordinator>() else {
        return;
    };
    let Some((session, generation)) = coordinator.take_for_cancel() else {
        // A stray cancel (e.g. during Freezing) must not destroy a window it does not own.
        log::info!("capture: cancel ignored (no live overlay session)");
        return;
    };
    cleanup_session(app, &session);
    coordinator.release_owned(generation);
    reveal::resume_after_capture(app, false);
    log::info!("capture: cancelled; no item or file was created");
}

/// Window-destroyed safety net: cleans a still-live session (Alt+F4, renderer crash).
///
/// The normal finish/cancel paths take the session first and release the coordinator, so this is a
/// logged no-op for them.
fn abandon_overlay(app: &AppHandle) {
    let Some(coordinator) = app.try_state::<CaptureCoordinator>() else {
        return;
    };
    let Some((session, generation)) = coordinator.take_for_cancel() else {
        log::debug!("capture: overlay window destroyed after the session was already handled");
        return;
    };
    cleanup_session(app, &session);
    coordinator.release_owned(generation);
    reveal::resume_after_capture(app, false);
    log::warn!("capture: the overlay window closed without confirm/cancel; capture cancelled");
}

/// Destroys the overlay window when it exists.
fn destroy_overlay(app: &AppHandle) {
    let Some(window) = app.get_webview_window(OVERLAY_LABEL) else {
        return;
    };
    if let Err(error) = window.destroy() {
        log::warn!("capture: failed to destroy the overlay window: {error}");
    }
}

/// Undoes one capture session: destroy the overlay and delete the frozen frame.
///
/// The line is deliberately not touched here: visibility and the pending tuck deadline belong to the
/// reveal controller (the single owner of `panel::show`/`hide`), which
/// [`reveal::resume_after_capture`] drives once the cleanup is done.
fn cleanup_session(app: &AppHandle, session: &CaptureSession) {
    destroy_overlay(app);
    match remove_file_if_exists(&session.temp_path) {
        Ok(true) => log::debug!(
            "capture: removed the frozen frame {}",
            session.temp_path.display()
        ),
        Ok(false) => {}
        Err(error) => log::warn!(
            "capture: cannot remove the frozen frame {}: {error}",
            session.temp_path.display()
        ),
    }
}

/// Converts a top-down 32-bit BGRA buffer into an opaque [`RgbaImage`].
///
/// GDI screen DIBs leave the alpha byte at 0; forcing it to 255 keeps the PNG opaque instead of
/// fully transparent. The B/R channels are swapped in place (no extra allocation).
fn rgba_from_bgra(mut bgra: Vec<u8>, width: u32, height: u32) -> anyhow::Result<RgbaImage> {
    if width == 0 || height == 0 {
        bail!("image dimensions must be non-zero, got {width}x{height}");
    }
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| anyhow!("image dimensions are too large: {width}x{height}"))?;
    if bgra.len() != expected {
        bail!(
            "BGRA buffer has {} bytes, expected {expected} for {width}x{height}",
            bgra.len()
        );
    }
    for pixel in bgra.chunks_exact_mut(4) {
        pixel.swap(0, 2);
        pixel[3] = 255;
    }
    RgbaImage::from_raw(width, height, bgra)
        .ok_or_else(|| anyhow!("failed to build an image from the GDI buffer"))
}

// ---------------------------------------------------------------------------------------------
// GDI RAII guards
// ---------------------------------------------------------------------------------------------

/// Screen DC from `GetDC(NULL)`, released with `ReleaseDC` on every exit path.
struct ScreenDc(HDC);

impl ScreenDc {
    fn acquire() -> anyhow::Result<Self> {
        let hdc = unsafe { GetDC(None) };
        if hdc.0.is_null() {
            bail!(
                "GetDC(NULL) failed: {}",
                std::io::Error::last_os_error()
            );
        }
        Ok(Self(hdc))
    }
}

impl Drop for ScreenDc {
    fn drop(&mut self) {
        // Returns the number of released DCs; nothing actionable on failure.
        let _ = unsafe { ReleaseDC(None, self.0) };
    }
}

/// Memory DC from `CreateCompatibleDC`, deleted with `DeleteDC` on every exit path.
struct MemoryDc(HDC);

impl MemoryDc {
    fn create(screen: HDC) -> anyhow::Result<Self> {
        let hdc = unsafe { CreateCompatibleDC(Some(screen)) };
        if hdc.0.is_null() {
            bail!(
                "CreateCompatibleDC failed: {}",
                std::io::Error::last_os_error()
            );
        }
        Ok(Self(hdc))
    }
}

impl Drop for MemoryDc {
    fn drop(&mut self) {
        let _ = unsafe { DeleteDC(self.0) };
    }
}

/// Compatible bitmap, deleted with `DeleteObject` on every exit path.
struct GdiBitmap(HBITMAP);

impl GdiBitmap {
    fn compatible(screen: HDC, width: i32, height: i32) -> anyhow::Result<Self> {
        let bitmap = unsafe { CreateCompatibleBitmap(screen, width, height) };
        if bitmap.0.is_null() {
            bail!(
                "CreateCompatibleBitmap({width}x{height}) failed: {}",
                std::io::Error::last_os_error()
            );
        }
        Ok(Self(bitmap))
    }
}

impl Drop for GdiBitmap {
    fn drop(&mut self) {
        let _ = unsafe { DeleteObject(HGDIOBJ(self.0.0)) };
    }
}

/// Restores the previously selected GDI object when it goes out of scope.
struct SelectGuard {
    dc: HDC,
    previous: HGDIOBJ,
}

impl SelectGuard {
    fn select(dc: HDC, object: HGDIOBJ) -> anyhow::Result<Self> {
        let previous = unsafe { SelectObject(dc, object) };
        // SelectObject returns NULL or HGDI_ERROR (-1) on failure.
        if previous.0.is_null() || previous.0 as isize == -1 {
            bail!(
                "SelectObject failed: {}",
                std::io::Error::last_os_error()
            );
        }
        Ok(Self { dc, previous })
    }
}

impl Drop for SelectGuard {
    fn drop(&mut self) {
        let _ = unsafe { SelectObject(self.dc, self.previous) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;
    use std::path::PathBuf;
    use std::sync::Once;

    use image::{Rgba, RgbaImage};
    use windows::Win32::Foundation::SYSTEMTIME;
    use windows::Win32::Graphics::Gdi::{GetDC, GetPixel, ReleaseDC};
    use windows::Win32::UI::HiDpi::{
        SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    };

    use crate::geometry::{self, PhysicalRect};

    /// Scratch directory under `target/` (git-ignored); never touches `%LOCALAPPDATA%`.
    fn test_dir(tag: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("capture-tests")
            .join(format!("{tag}-{}", std::process::id()))
    }

    /// Makes the test process per-monitor DPI aware before any monitor/GDI call.
    ///
    /// `cargo test` binaries have no DPI manifest, so without this Windows reports virtualized
    /// (DPI-unaware) monitor rectangles and a 150% monitor would look like 96 DPI.
    fn ensure_per_monitor_dpi_aware() {
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            match unsafe {
                SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)
            } {
                Ok(()) => eprintln!("E5: process DPI awareness = PerMonitorV2"),
                Err(err) => eprintln!(
                    "E5: SetProcessDpiAwarenessContext failed (already set by a manifest?): {err}"
                ),
            }
        });
    }

    /// Reads one physical screen pixel through `GetPixel` (independent of the BitBlt pipeline).
    ///
    /// `GetPixel` returns a `COLORREF` (`0x00BBGGRR`); this converts it to `(r, g, b)`.
    fn screen_pixel(x: i32, y: i32) -> (u8, u8, u8) {
        let hdc = unsafe { GetDC(None) };
        assert!(!hdc.0.is_null(), "GetDC(NULL) failed");
        let color = unsafe { GetPixel(hdc, x, y) };
        unsafe {
            ReleaseDC(None, hdc);
        }
        assert_ne!(
            color.0, 0xFFFF_FFFF,
            "GetPixel failed at physical ({x}, {y})"
        );
        (
            (color.0 & 0xFF) as u8,
            ((color.0 >> 8) & 0xFF) as u8,
            ((color.0 >> 16) & 0xFF) as u8,
        )
    }

    #[test]
    fn bgra_rows_convert_to_rgba_and_force_opaque_alpha() {
        // 32-bit GDI DIBs store BGRA and leave the alpha byte at 0 for screen captures.
        let raw = vec![10, 20, 30, 0, 40, 50, 60, 128];
        let image = rgba_from_bgra(raw, 2, 1).expect("valid 2x1 BGRA buffer");
        assert_eq!(image.dimensions(), (2, 1));
        assert_eq!(image.get_pixel(0, 0).0, [30, 20, 10, 255]);
        assert_eq!(image.get_pixel(1, 0).0, [60, 50, 40, 255]);

        // A buffer whose length does not match the dimensions is rejected, not truncated.
        assert!(rgba_from_bgra(vec![0, 0, 0], 1, 1).is_err());
        assert!(rgba_from_bgra(Vec::new(), 0, 1).is_err());
    }

    #[test]
    fn crop_extracts_the_requested_region() {
        let image = RgbaImage::from_fn(4, 3, |x, y| Rgba([x as u8, y as u8, 7, 255]));
        let cropped = crop(
            &image,
            PhysicalRect {
                x: 1,
                y: 1,
                w: 2,
                h: 2,
            },
        );
        assert_eq!(cropped.dimensions(), (2, 2));
        for y in 0..2 {
            for x in 0..2 {
                assert_eq!(
                    cropped.get_pixel(x, y).0,
                    image.get_pixel(x + 1, y + 1).0,
                    "crop pixel ({x},{y}) must come from source ({},{})",
                    x + 1,
                    y + 1
                );
            }
        }
    }

    #[test]
    fn crop_clamps_negative_and_oversized_rects() {
        let image = RgbaImage::from_fn(4, 3, |x, y| Rgba([x as u8, y as u8, 7, 255]));

        // Rectangle starts left/above the image and ends inside it.
        let cropped = crop(
            &image,
            PhysicalRect {
                x: -2,
                y: -1,
                w: 4,
                h: 3,
            },
        );
        assert_eq!(cropped.dimensions(), (2, 2));
        assert_eq!(cropped.get_pixel(0, 0).0, image.get_pixel(0, 0).0);
        assert_eq!(cropped.get_pixel(1, 1).0, image.get_pixel(1, 1).0);

        // Rectangle starts inside and extends past the right/bottom edge.
        let cropped = crop(
            &image,
            PhysicalRect {
                x: 3,
                y: 2,
                w: 100,
                h: 100,
            },
        );
        assert_eq!(cropped.dimensions(), (1, 1));
        assert_eq!(cropped.get_pixel(0, 0).0, image.get_pixel(3, 2).0);
    }

    #[test]
    fn crop_fully_outside_or_degenerate_is_empty() {
        let image = RgbaImage::from_fn(4, 3, |_, _| Rgba([1, 2, 3, 255]));
        for rect in [
            PhysicalRect {
                x: 10,
                y: 10,
                w: 5,
                h: 5,
            },
            PhysicalRect {
                x: -10,
                y: -10,
                w: 5,
                h: 5,
            },
            PhysicalRect {
                x: 1,
                y: 1,
                w: 0,
                h: 0,
            },
            PhysicalRect {
                x: 1,
                y: 1,
                w: -3,
                h: -3,
            },
            PhysicalRect {
                x: i32::MAX,
                y: 0,
                w: i32::MAX,
                h: 1,
            },
        ] {
            assert_eq!(
                crop(&image, rect).dimensions(),
                (0, 0),
                "rect {rect:?} must crop to an empty image"
            );
        }
    }

    #[test]
    fn png_filename_uses_local_time_to_the_second() {
        let local = SYSTEMTIME {
            wYear: 2026,
            wMonth: 10,
            wDay: 29,
            wHour: 13,
            wMinute: 5,
            wSecond: 9,
            ..Default::default()
        };
        assert_eq!(png_filename(local), "Hangline 2026-10-29 13.05.09.png");
    }

    #[test]
    fn save_png_writes_atomically_and_reloads() {
        let dir = test_dir("save");
        let image = RgbaImage::from_fn(7, 5, |x, y| {
            Rgba([(x * 30) as u8, (y * 40) as u8, 128, 255])
        });
        let path = save_png_to(&dir, &image).expect("save screenshot");

        assert_eq!(path.parent(), Some(dir.as_path()));
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .expect("file name");
        assert!(name.starts_with("Hangline "), "unexpected name: {name}");
        assert!(name.ends_with(".png"), "unexpected name: {name}");
        assert_eq!(name.len(), "Hangline 2026-10-29 13.05.09.png".len());
        assert!(
            !dir.join(format!("{name}.tmp")).exists(),
            "the .tmp file must be renamed away"
        );

        // The PNG must round-trip byte-identically at the pixel level (no channel swaps).
        let loaded = image::open(&path)
            .expect("reload screenshot")
            .to_rgba8();
        assert_eq!(loaded, image);

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn screenshots_dir_is_localappdata_hangline_screenshots() {
        let dir = screenshots_dir().expect("screenshots dir");
        let local_app_data = std::env::var_os("LOCALAPPDATA").expect("LOCALAPPDATA is set");
        assert_eq!(
            dir,
            PathBuf::from(local_app_data)
                .join("Hangline")
                .join("Screenshots")
        );
        assert!(dir.is_dir(), "screenshots_dir must create the directory");
    }

    #[test]
    #[ignore = "E5 runtime evidence: requires this machine's dual-monitor session (1920x1080@100% + left 1920x1080@150%)"]
    fn e5_real_session_monitors() {
        ensure_per_monitor_dpi_aware();

        let monitors = geometry::monitors();
        assert_eq!(monitors.len(), 2, "E5 rig must expose two monitors");

        let left = monitors
            .iter()
            .find(|monitor| monitor.rect.x < 0)
            .expect("left secondary monitor");
        assert_eq!(
            left.rect,
            PhysicalRect {
                x: -1920,
                y: 0,
                w: 1920,
                h: 1080,
            }
        );
        assert!(!left.is_primary);
        assert!(
            (left.scale_factor - 1.5).abs() < f64::EPSILON,
            "left monitor must be 150%, got {}",
            left.scale_factor
        );
        assert_eq!(
            left.work_area,
            PhysicalRect {
                x: -1920,
                y: 0,
                w: 1920,
                h: 1008,
            }
        );

        let primary = monitors
            .iter()
            .find(|monitor| monitor.is_primary)
            .expect("primary monitor");
        assert_eq!(
            primary.rect,
            PhysicalRect {
                x: 0,
                y: 0,
                w: 1920,
                h: 1080,
            }
        );
        assert!((primary.scale_factor - 1.0).abs() < f64::EPSILON);
        assert_eq!(
            primary.work_area,
            PhysicalRect {
                x: 0,
                y: 0,
                w: 1920,
                h: 1032,
            }
        );

        // Live monitor-at-point / work-area selection, including the negative-x monitor.
        let at_left = geometry::monitor_at((-1000, 500)).expect("monitor at (-1000,500)");
        assert_eq!(at_left.id, left.id);
        assert_eq!(
            geometry::work_area_of(&monitors, (-1000, 500)),
            Some(left.work_area)
        );
        assert_eq!(
            geometry::monitor_at((1000, 500)).map(|monitor| monitor.id),
            Some(primary.id.clone())
        );

        // Physical → logical at the real scale factors: 1920x1080 @150% is 1280x720 dp.
        let left_logical = geometry::to_logical(left.rect, left.scale_factor);
        assert!((left_logical.w - 1280.0).abs() < 0.01);
        assert!((left_logical.h - 720.0).abs() < 0.01);
        let left_work_logical = geometry::to_logical(left.work_area, left.scale_factor);
        assert!((left_work_logical.w - 1280.0).abs() < 0.01);
        assert!((left_work_logical.h - 672.0).abs() < 0.01);
        // The same 48 dp taskbar is 48 px at 100% and 72 px at 150%.
        assert_eq!(primary.rect.h - primary.work_area.h, 48);
        assert_eq!(left.rect.h - left.work_area.h, 72);

        println!("E5 monitors: {monitors:?}");
    }

    #[test]
    #[ignore = "E5 runtime evidence: run from a binary copied outside the workspace (the sandbox blocks %LOCALAPPDATA% writes from target/); requires the dual-monitor session"]
    fn e5_virtual_screen_capture_crops_and_pixels() {
        ensure_per_monitor_dpi_aware();

        let screen = virtual_screen_rect();
        assert_eq!(
            screen,
            PhysicalRect {
                x: -1920,
                y: 0,
                w: 3840,
                h: 1080,
            },
            "virtual screen must span the 150% left monitor and the 100% primary"
        );

        let image = grab_virtual_screen().expect("GDI virtual-screen capture");
        assert_eq!(image.dimensions(), (3840, 1080));
        assert!(
            image.pixels().all(|pixel| pixel.0[3] == 255),
            "screen captures must be fully opaque"
        );

        // Compare the BitBlt/BGRA pipeline against an independent GDI read of the same pixels.
        // Sampling both monitors (and the seam) catches channel swaps and coordinate offsets.
        let mut probes = 0usize;
        for (x, y) in [
            (-1920, 0),
            (-1919, 1079),
            (-1000, 500),
            (-1, 1079),
            (0, 0),
            (1, 1),
            (960, 540),
            (1919, 1079),
        ] {
            let expected = screen_pixel(x, y);
            let pixel = image.get_pixel(
                (x - screen.x) as u32,
                (y - screen.y) as u32,
            );
            assert_eq!(
                (pixel.0[0], pixel.0[1], pixel.0[2]),
                expected,
                "captured pixel at physical ({x},{y}) must match GetPixel"
            );
            probes += 1;
        }

        // A completely black frame would make the seam checks meaningless; this rig has the
        // lock-screen wallpaper on the primary monitor even while the secondary stays black.
        assert!(
            image.pixels().any(|pixel| pixel.0[..3] != [0, 0, 0]),
            "the primary monitor must contribute non-black pixels"
        );

        let full_path = save_png(&image).expect("save the virtual-screen PNG");
        let full_bytes = fs::read(&full_path).expect("read the saved PNG");
        let full_hash = blake3::hash(&full_bytes).to_hex().to_string();
        let reloaded = image::open(&full_path)
            .expect("reload the saved PNG")
            .to_rgba8();
        assert_eq!(reloaded, image, "saved PNG must round-trip pixel-exactly");

        let evidence_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("e5-evidence");
        fs::create_dir_all(&evidence_dir).expect("create evidence dir");
        let left_crop = crop(
            &image,
            PhysicalRect {
                x: 0,
                y: 0,
                w: 1920,
                h: 1080,
            },
        );
        let primary_crop = crop(
            &image,
            PhysicalRect {
                x: 1920,
                y: 0,
                w: 1920,
                h: 1080,
            },
        );
        let seam_crop = crop(
            &image,
            PhysicalRect {
                x: 1920 - 320,
                y: 0,
                w: 640,
                h: 360,
            },
        );
        assert_eq!(left_crop.dimensions(), (1920, 1080));
        assert_eq!(primary_crop.dimensions(), (1920, 1080));
        assert_eq!(seam_crop.dimensions(), (640, 360));

        let mut crop_hashes = Vec::new();
        for (name, crop_image) in [
            ("left-monitor-150pct.png", &left_crop),
            ("primary-monitor-100pct.png", &primary_crop),
            ("seam-x0-640x360.png", &seam_crop),
        ] {
            let path = evidence_dir.join(name);
            crop_image
                .save_with_format(&path, image::ImageFormat::Png)
                .expect("save crop");
            let hash = blake3::hash(&fs::read(&path).expect("read crop"))
                .to_hex()
                .to_string();
            crop_hashes.push((path, hash));
        }

        println!(
            "E5 capture: {}x{} at ({},{}), probes={probes}, png={} ({} bytes) blake3={full_hash}",
            image.width(),
            image.height(),
            screen.x,
            screen.y,
            full_path.display(),
            full_bytes.len()
        );
        for (path, hash) in crop_hashes {
            println!("E5 crop: {} blake3={hash}", path.display());
        }
    }

    // -----------------------------------------------------------------------------------------
    // Task 16: virtual-screen crop, own-capture naming/claim order and the overlay session state
    // -----------------------------------------------------------------------------------------

    /// An image whose pixels encode their own coordinates (a wrong offset is visible in asserts).
    fn coordinate_image(width: u32, height: u32) -> RgbaImage {
        RgbaImage::from_fn(width, height, |x, y| {
            Rgba([(x % 251) as u8, (y % 251) as u8, 9, 255])
        })
    }

    /// A frozen frame for `screen` and a synthetic virtual desktop with a negative origin.
    ///
    /// The left secondary monitor of the P1 rig sits at `x = -1920` (150%); the synthetic case
    /// keeps the arithmetic identical without allocating a 3840x1080 image in every test.
    const NEGATIVE_ORIGIN_SCREEN: PhysicalRect = PhysicalRect {
        x: -120,
        y: -30,
        w: 200,
        h: 100,
    };

    /// A capture session with fixed metadata (the image content is irrelevant to the decisions).
    fn session() -> CaptureSession {
        CaptureSession {
            temp_path: PathBuf::from(r"C:\Users\Hangline\AppData\Local\Hangline\capture-tmp.png"),
            virtual_rect: PhysicalRect {
                x: -1920,
                y: 0,
                w: 3840,
                h: 1080,
            },
            image: RgbaImage::new(4, 3),
            scale: 1.5,
        }
    }

    /// A clean scratch directory (exact-name assertions must not see a previous run's files).
    fn fresh_test_dir(tag: &str) -> PathBuf {
        let dir = test_dir(tag);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create the fresh test dir");
        dir
    }

    /// The fixed local time used by the same-second naming tests.
    fn test_local_time() -> SYSTEMTIME {
        SYSTEMTIME {
            wYear: 2026,
            wMonth: 10,
            wDay: 29,
            wHour: 13,
            wMinute: 5,
            wSecond: 9,
            ..Default::default()
        }
    }

    #[test]
    fn crop_virtual_maps_a_negative_virtual_origin_into_image_pixels() {
        let image = coordinate_image(200, 100);

        // A region 20 px / 10 px into the frozen image, expressed in virtual-desktop coordinates.
        let cropped = crop_virtual_with_origin(
            &image,
            PhysicalRect {
                x: -100,
                y: -20,
                w: 50,
                h: 40,
            },
            NEGATIVE_ORIGIN_SCREEN,
        );
        assert_eq!(cropped.dimensions(), (50, 40));
        assert_eq!(cropped.get_pixel(0, 0).0, image.get_pixel(20, 10).0);
        assert_eq!(cropped.get_pixel(49, 39).0, image.get_pixel(69, 49).0);
    }

    #[test]
    fn crop_virtual_clamps_across_a_negative_origin_to_the_frozen_frame() {
        let image = coordinate_image(200, 100);

        // Starts above/left of the captured frame and is clamped to the top-left image corner.
        let clamped = crop_virtual_with_origin(
            &image,
            PhysicalRect {
                x: -150,
                y: -60,
                w: 60,
                h: 60,
            },
            NEGATIVE_ORIGIN_SCREEN,
        );
        assert_eq!(clamped.dimensions(), (30, 30));
        assert_eq!(clamped.get_pixel(0, 0).0, image.get_pixel(0, 0).0);
        assert_eq!(clamped.get_pixel(29, 29).0, image.get_pixel(29, 29).0);

        // Starts inside (image 100,70) and extends past the bottom-right edge: 100x30 kept.
        let oversized = crop_virtual_with_origin(
            &image,
            PhysicalRect {
                x: -20,
                y: 40,
                w: 100,
                h: 100,
            },
            NEGATIVE_ORIGIN_SCREEN,
        );
        assert_eq!(oversized.dimensions(), (100, 30));
        assert_eq!(oversized.get_pixel(0, 0).0, image.get_pixel(100, 70).0);

        // Fully outside the image / degenerate selections produce an empty image (never a panic).
        for rect in [
            PhysicalRect {
                x: -500,
                y: -500,
                w: 10,
                h: 10,
            },
            PhysicalRect {
                x: 0,
                y: 0,
                w: 0,
                h: 10,
            },
            PhysicalRect {
                x: 0,
                y: 0,
                w: -5,
                h: 5,
            },
        ] {
            assert_eq!(
                crop_virtual_with_origin(&image, rect, NEGATIVE_ORIGIN_SCREEN).dimensions(),
                (0, 0),
                "rect {rect:?} must crop to an empty image"
            );
        }
    }

    #[test]
    fn crop_virtual_uses_the_live_virtual_screen_origin() {
        // On the dual-monitor rig the origin is negative (the 150% monitor sits at x = -1920);
        // expressing the expected rectangle relative to that origin keeps the test valid anywhere.
        let screen = virtual_screen_rect();
        let image = coordinate_image(64, 48);
        let rect = PhysicalRect {
            x: screen.x.saturating_add(10),
            y: screen.y.saturating_add(5),
            w: 20,
            h: 15,
        };
        let cropped = crop_virtual(&image, rect);
        assert_eq!(cropped.dimensions(), (20, 15));
        assert_eq!(cropped.get_pixel(0, 0).0, image.get_pixel(10, 5).0);
        assert_eq!(cropped.get_pixel(19, 14).0, image.get_pixel(29, 19).0);
    }

    #[test]
    fn selections_need_a_positive_physical_size() {
        assert!(is_valid_selection(PhysicalRect {
            x: 0,
            y: 0,
            w: 1,
            h: 1
        }));
        assert!(is_valid_selection(PhysicalRect {
            x: -1920,
            y: 0,
            w: 100,
            h: 80
        }));
        assert!(!is_valid_selection(PhysicalRect {
            x: 0,
            y: 0,
            w: 0,
            h: 80
        }));
        assert!(!is_valid_selection(PhysicalRect {
            x: 0,
            y: 0,
            w: 100,
            h: -1
        }));
    }

    #[test]
    fn unique_png_path_numbers_same_second_collisions_like_clipboard_saves() {
        let dir = fresh_test_dir("unique");
        let stem = "Hangline 2026-10-29 13.05.09";

        let first = unique_png_path(&dir, stem);
        assert_eq!(first, dir.join(format!("{stem}.png")));
        fs::write(&first, b"first").expect("write first name");

        let second = unique_png_path(&dir, stem);
        assert_eq!(second, dir.join(format!("{stem}-2.png")));
        fs::write(&second, b"second").expect("write second name");

        assert_eq!(unique_png_path(&dir, stem), dir.join(format!("{stem}-3.png")));

        // Every numbered name taken: the millisecond fallback never overwrites an existing file.
        for index in 4..=MAX_NAME_SUFFIX {
            fs::write(dir.join(format!("{stem}-{index}.png")), b"taken").expect("write taken name");
        }
        let fallback = unique_png_path(&dir, stem);
        assert!(!fallback.exists(), "the fallback must be a free path");
        assert!(fallback
            .file_name()
            .and_then(|name| name.to_str())
            .expect("file name")
            .starts_with(&format!("{stem}-")));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn reservation_never_replaces_a_concurrent_file() {
        let dir = fresh_test_dir("reserve");
        let stem = "Hangline 2026-10-29 13.05.09";

        // Another writer (e.g. the clipboard saver) already owns the plain name.
        let taken = dir.join(format!("{stem}.png"));
        fs::write(&taken, b"clipboard image").expect("write the concurrent file");
        let reserved = reserve_capture_path(&dir, stem).expect("reservation");
        assert_eq!(reserved, dir.join(format!("{stem}-2.png")));
        assert_eq!(
            fs::read(&taken).expect("read the concurrent file"),
            b"clipboard image",
            "an existing same-second file must never be replaced"
        );
        // A second reservation takes the next free name; `create_new` makes them mutually exclusive.
        let another = reserve_capture_path(&dir, stem).expect("second reservation");
        assert_eq!(another, dir.join(format!("{stem}-3.png")));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn own_capture_claims_the_exact_file_hash_before_the_rename() {
        let dir = fresh_test_dir("claim");
        let image = RgbaImage::from_fn(5, 4, |x, y| Rgba([(x * 40) as u8, (y * 50) as u8, 200, 255]));
        let deduper = crate::dedupe::Deduper::default();
        let local = test_local_time();

        let mut steps = Vec::new();
        let mut content_visible_before_claim = true;
        let mut claim_existed_before_rename = false;
        let saved = save_capture_image_observed(&dir, &image, &deduper, local, |step, path| {
            steps.push(step);
            match step {
                CaptureSaveStep::Reserved => {
                    // The reservation is an empty placeholder, not the screenshot content.
                    assert!(path.is_file(), "the final name must be reserved");
                    assert_eq!(
                        fs::metadata(path).expect("reservation metadata").len(),
                        0,
                        "the reservation must be empty"
                    );
                }
                CaptureSaveStep::Hashed => {
                    content_visible_before_claim =
                        fs::metadata(path).expect("metadata").len() > 0;
                }
                CaptureSaveStep::Claimed => {
                    // The claim exists while only the empty reservation is visible: the content-
                    // bearing file can never be observed by the watcher before the claim.
                    claim_existed_before_rename = deduper.claim_count() == 1;
                    assert_eq!(
                        fs::metadata(path).expect("metadata").len(),
                        0,
                        "the PNG bytes must not be visible before the claim"
                    );
                }
                _ => {}
            }
        })
        .expect("save the own capture");

        assert_eq!(
            steps,
            vec![
                CaptureSaveStep::Reserved,
                CaptureSaveStep::TmpWritten,
                CaptureSaveStep::Hashed,
                CaptureSaveStep::Claimed,
                CaptureSaveStep::Renamed,
            ],
            "the Task 8 order is reserved -> tmp -> hash -> claim -> rename"
        );
        assert!(!content_visible_before_claim);
        assert!(claim_existed_before_rename);

        let final_path = dir.join("Hangline 2026-10-29 13.05.09.png");
        assert_eq!(saved.path, final_path);
        assert_eq!((saved.width, saved.height), (5, 4));
        assert!(
            !dir.join("Hangline 2026-10-29 13.05.09.png.tmp").exists(),
            "the .png.tmp must be renamed away"
        );
        assert!(
            fs::metadata(&saved.path).expect("final metadata").len() > 0,
            "the reservation must be replaced by the PNG"
        );

        // The claimed hash is the hash of the exact bytes on disk: the rename does not change them,
        // so the folder watcher (which hashes the renamed file) sees the same value.
        assert_eq!(
            crate::dedupe::hash_file(&saved.path).expect("hash the saved file"),
            saved.hash
        );
        // The pending claim suppresses the own-captures folder watcher exactly once...
        assert!(
            !deduper.check_or_insert(saved.hash),
            "the watcher event for the new file must be deduped"
        );
        // ... and the hash stays in the durable LRU for later repeats.
        assert!(!deduper.check_or_insert(saved.hash));

        let reloaded = image::open(&saved.path)
            .expect("reload the saved capture")
            .to_rgba8();
        assert_eq!(reloaded, image);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn same_second_captures_are_both_kept_with_a_numbered_suffix() {
        let dir = fresh_test_dir("same-second");
        let image = coordinate_image(6, 3);
        let deduper = crate::dedupe::Deduper::default();
        let local = test_local_time();

        let first = save_capture_image_at(&dir, &image, &deduper, local).expect("first capture");
        let second = save_capture_image_at(&dir, &image, &deduper, local).expect("second capture");
        assert_ne!(first.path, second.path, "same-second captures never overwrite");
        assert_eq!(
            second.path,
            dir.join("Hangline 2026-10-29 13.05.09-2.png")
        );
        assert!(first.path.is_file() && second.path.is_file());
        assert_eq!(first.hash, second.hash, "identical pixels have identical bytes");

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn concurrent_same_second_saves_reserve_distinct_names() {
        let dir = fresh_test_dir("concurrent");
        let deduper = std::sync::Arc::new(crate::dedupe::Deduper::default());
        let local = test_local_time();
        let mut handles = Vec::new();
        for index in 0..8u32 {
            let dir = dir.clone();
            let deduper = std::sync::Arc::clone(&deduper);
            handles.push(std::thread::spawn(move || {
                let image = RgbaImage::from_fn(6, 3, |x, y| {
                    Rgba([
                        (x + index) as u8,
                        (y + index) as u8,
                        index as u8,
                        255,
                    ])
                });
                save_capture_image_at(&dir, &image, &deduper, local).expect("concurrent save")
            }));
        }
        let saved: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().expect("join the save thread"))
            .collect();
        let paths: std::collections::HashSet<_> =
            saved.iter().map(|capture| capture.path.clone()).collect();
        assert_eq!(
            paths.len(),
            saved.len(),
            "every concurrent same-second save must get its own reserved name"
        );
        for capture in &saved {
            assert!(capture.path.is_file());
            assert!(fs::metadata(&capture.path).expect("metadata").len() > 0);
            assert_eq!(
                crate::dedupe::hash_file(&capture.path).expect("hash"),
                capture.hash
            );
        }
        let leftovers: Vec<_> = fs::read_dir(&dir)
            .expect("read the test dir")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "no .tmp left behind: {leftovers:?}");
        assert_eq!(deduper.len(), saved.len(), "every saved content hash is remembered");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn stale_frozen_frames_are_removed_only_when_present() {
        let dir = fresh_test_dir("stale");
        let path = dir.join("capture-tmp.png");
        assert_eq!(
            remove_file_if_exists(&path).expect("missing file is not an error"),
            false
        );
        fs::write(&path, b"stale frozen frame").expect("write a stale frozen frame");
        assert_eq!(remove_file_if_exists(&path).expect("remove"), true);
        assert!(!path.exists(), "the crash residue must be gone");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn the_preflight_wait_returns_as_soon_as_the_window_is_hidden() {
        // Covers both the line (`missing_is_hidden = false`) and the fly overlay (true): a hidden
        // window is the fast common case and must not burn the timeout.
        for missing_is_hidden in [false, true] {
            let started = Instant::now();
            wait_until_window_hidden("a test window", || Some(false), missing_is_hidden)
                .expect("a hidden window passes");
            assert!(
                started.elapsed() < PANEL_HIDE_TIMEOUT,
                "a hidden window must return before the timeout"
            );
        }
    }

    #[test]
    fn the_preflight_wait_accepts_a_missing_optional_window() {
        // The fly overlay is created lazily: no window is the normal state for a pre-warmed overlay
        // that failed to create, so the freeze must proceed.
        wait_until_window_hidden("the fly overlay", || None, true)
            .expect("a missing optional window counts as hidden");
    }

    #[test]
    fn the_preflight_wait_aborts_on_a_missing_required_window() {
        // The line always exists while the app runs; an unreadable panel must abort the freeze
        // rather than grab a screen that may still show the line.
        let error = wait_until_window_hidden("the line", || None, false)
            .expect_err("a missing line window aborts the freeze");
        assert!(
            error.to_string().contains("cannot read the visibility of the line"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn the_preflight_wait_aborts_when_the_window_never_hides() {
        let started = Instant::now();
        let error = wait_until_window_hidden("the fly overlay", || Some(true), true)
            .expect_err("a window that stays visible aborts the freeze");
        assert!(
            started.elapsed() >= PANEL_HIDE_TIMEOUT,
            "the wait must poll for the whole timeout before aborting"
        );
        assert!(
            error
                .to_string()
                .contains("the fly overlay is still visible after 240 ms; aborting the freeze"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn a_missing_overlay_session_resets_to_idle_instead_of_blocking_captures() {
        let coordinator = CaptureCoordinator::default();
        let lease = coordinator.try_begin().expect("begin the request");
        assert!(coordinator.attach_overlay(lease, session()));
        coordinator.force_overlay_without_session();
        assert!(
            coordinator.take_for_finish().is_none(),
            "a broken overlay invariant must not yield a session"
        );
        assert_eq!(
            coordinator.phase(),
            CapturePhase::Idle,
            "the defensive reset must unblock future captures"
        );
        assert!(coordinator.try_begin().is_some());
    }

    #[test]
    fn the_coordinator_blocks_reentry_with_an_owned_lease() {
        let coordinator = CaptureCoordinator::default();
        let lease = coordinator.try_begin().expect("the first request starts the freeze");
        assert!(
            coordinator.try_begin().is_none(),
            "a second capture request must not run while one is in flight"
        );
        assert_eq!(coordinator.phase(), CapturePhase::Freezing);
        assert!(coordinator.attach_overlay(lease, session()));
        assert_eq!(coordinator.phase(), CapturePhase::Overlay);
        let (taken, generation) = coordinator
            .take_for_finish()
            .expect("the live overlay session");
        assert_eq!(taken.image.width(), 4);
        assert_eq!(coordinator.phase(), CapturePhase::Cleaning);
        assert!(
            coordinator.try_begin().is_none(),
            "cleanup still holds the reentry guard"
        );
        assert!(
            coordinator.take_for_finish().is_none(),
            "the session is taken exactly once (double finish is rejected)"
        );
        coordinator.release_owned(generation);
        assert_eq!(coordinator.phase(), CapturePhase::Idle);
        assert!(
            coordinator.try_begin().is_some(),
            "after release a new capture may start"
        );
    }

    #[test]
    fn a_stale_request_cannot_wipe_a_newer_capture() {
        let coordinator = CaptureCoordinator::default();
        let stale = coordinator.try_begin().expect("first request");
        // A stray cancel during Freezing must be a no-op: the overlay does not exist yet.
        assert!(coordinator.take_for_cancel().is_none());
        assert_eq!(coordinator.phase(), CapturePhase::Freezing);
        // The first request fails and aborts (it is still the owner, so this releases the guard).
        assert!(coordinator.abort_owned(stale).is_none());
        assert_eq!(coordinator.phase(), CapturePhase::Idle);

        // A new request starts and attaches its own session.
        let fresh = coordinator.try_begin().expect("second request");
        assert!(coordinator.attach_overlay(fresh, session()));
        // The old owner's late cleanup is ignored: it cannot clear the new phase/session.
        assert!(coordinator.abort_owned(stale).is_none());
        coordinator.release_owned(stale.generation);
        assert_eq!(
            coordinator.phase(),
            CapturePhase::Overlay,
            "the stale owner must not release the new capture"
        );
        assert!(
            coordinator.ready_payload().is_some(),
            "the new session must survive the stale cleanup"
        );
        // The new owner can finish normally.
        let (_, generation) = coordinator.take_for_finish().expect("new session");
        coordinator.release_owned(generation);
        assert_eq!(coordinator.phase(), CapturePhase::Idle);
    }

    #[test]
    fn cancel_only_takes_a_live_overlay_and_releases_its_owner() {
        let coordinator = CaptureCoordinator::default();
        let lease = coordinator.try_begin().expect("begin the request");
        assert!(
            coordinator.take_for_cancel().is_none(),
            "a request that never reached the overlay has no session to clean"
        );
        assert!(coordinator.attach_overlay(lease, session()));
        let (taken, generation) = coordinator
            .take_for_cancel()
            .expect("live overlay session");
        assert_eq!(taken.image.width(), 4);
        assert_eq!(coordinator.phase(), CapturePhase::Cleaning);
        assert!(coordinator.try_begin().is_none());
        coordinator.release_owned(generation);
        assert_eq!(coordinator.phase(), CapturePhase::Idle);
        assert!(coordinator.try_begin().is_some());
    }

    #[test]
    fn capture_ready_payload_serializes_for_the_overlay_frontend() {
        let payload = session().ready_payload();
        assert_eq!(
            serde_json::to_value(&payload).expect("serialize capture-ready"),
            serde_json::json!({
                "tempPath": r"C:\Users\Hangline\AppData\Local\Hangline\capture-tmp.png",
                "virtualRect": { "x": -1920, "y": 0, "w": 3840, "h": 1080 },
                "scale": 1.5,
                "width": 4,
                "height": 3,
            })
        );
    }

    #[test]
    fn the_frozen_frame_temp_file_is_inside_the_asset_scope() {
        let local_app_data = std::env::var_os("LOCALAPPDATA").expect("LOCALAPPDATA is set");
        assert_eq!(CAPTURE_TMP_FILE, "capture-tmp.png");
        assert_eq!(
            capture_tmp_path().expect("frozen frame path"),
            PathBuf::from(local_app_data)
                .join("Hangline")
                .join(CAPTURE_TMP_FILE)
        );
    }

    #[test]
    fn overlay_url_carries_the_raw_theme_setting() {
        // The capture window is a separate webview: Rust must hand it the raw `theme` setting so it
        // can resolve `system` itself (P2-8 review: this wire value silently degrading to system
        // would disable forced light/dark for the overlay, so every variant is asserted).
        for (theme, wire) in [
            (Theme::System, "system"),
            (Theme::Light, "light"),
            (Theme::Dark, "dark"),
        ] {
            let url = capture_url(theme);
            assert!(url.starts_with("index.html?window=capture"), "url: {url}");
            assert!(url.contains(&format!("theme={wire}")), "url: {url}");
        }
    }
}
