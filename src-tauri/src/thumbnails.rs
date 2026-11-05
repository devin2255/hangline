//! P2-7 thumbnail cache: keeps small previews for large screenshots so the panel never decodes a
//! 4K PNG per card.
//!
//! The cache lives under `%LOCALAPPDATA%\Hangline\Thumbnails\<item-id>.png` (inside the existing
//! asset-scope root) and each thumb is a PNG whose longest edge is [`THUMB_MAX_EDGE`] px (smaller
//! sources are copied, never upscaled). Writes go through a `.png.tmp` file + rename so a reader
//! can never see a half-written preview.
//!
//! Freshness is explicit: a thumb is fresh only when its mtime is strictly newer than the source's;
//! equal mtimes are treated as stale because a coarse filesystem cannot prove ordering and
//! rebuilding the 432 px preview is cheaper than showing a pre-edit image. A mtime-fresh preview is
//! additionally decode-checked (P4-6 self-heal): a zero-byte or truncated cache file is rebuilt
//! once from the source instead of being returned forever. Orphaned cache files (items removed,
//! tampered ids, stale temp files) are swept on startup.
//!
//! Recordings get their first frame from the WinRT media stack (`crate::video`, P3-5) and reuse
//! the same 432 px atomic writer; a failed decode keeps the card's filename tile + play badge.
//! Image decode errors (HEIC, corrupt files) are logged and treated as "no thumbnail" instead of
//! panicking.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::Context;
use image::RgbaImage;
use tauri::{AppHandle, Manager};

use crate::app_state::{self, AppState};
use crate::item::{Item, ItemKind};

/// Longest edge of a cached thumbnail in pixels (`design plan P2-7`).
pub const THUMB_MAX_EDGE: u32 = 432;

/// Recording extensions handled by the WinRT first-frame helper instead of `image::open`.
const RECORDING_EXTENSIONS: &[&str] = &["mp4", "mov", "mkv", "webm", "avi", "wmv"];

/// One generated/reused thumbnail plus the decoded source dimensions when known.
#[derive(Clone, Debug)]
pub struct Thumb {
    /// `<cache>/<id>.png`.
    pub path: PathBuf,
    /// Decoded source width (0 when the header could not be read).
    pub width: u32,
    /// Decoded source height (0 when the header could not be read).
    pub height: u32,
}

/// A file's freshness stamp (missing/unreadable files have no stamp).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FileStamp {
    /// Last modification time, when the filesystem exposes it.
    pub modified: Option<SystemTime>,
}

/// Test-only cache-directory override so state-layer tests never touch `%LOCALAPPDATA%`.
#[cfg(test)]
static TEST_CACHE_DIR: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

/// Test-only setter for [`TEST_CACHE_DIR`]; `None` restores the real cache directory.
#[cfg(test)]
pub(crate) fn set_test_cache_dir(dir: Option<PathBuf>) {
    *TEST_CACHE_DIR
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = dir;
}

/// RAII guard restoring the real cache directory even when the test panics.
#[cfg(test)]
pub(crate) struct TestCacheDirGuard;

#[cfg(test)]
impl Drop for TestCacheDirGuard {
    fn drop(&mut self) {
        set_test_cache_dir(None);
    }
}

/// Points the test-only cache override at `dir` until the returned guard is dropped.
#[cfg(test)]
pub(crate) fn use_test_cache_dir(dir: PathBuf) -> TestCacheDirGuard {
    set_test_cache_dir(Some(dir));
    TestCacheDirGuard
}

/// `%LOCALAPPDATA%\Hangline\Thumbnails`, created on demand.
pub fn cache_dir() -> anyhow::Result<PathBuf> {
    #[cfg(test)]
    if let Some(dir) = TEST_CACHE_DIR
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
    {
        fs::create_dir_all(&dir).ok();
        return Ok(dir);
    }
    let dir = crate::settings::data_dir()
        .context("thumbnails: the data directory is unavailable")?
        .join("Thumbnails");
    fs::create_dir_all(&dir).with_context(|| {
        format!(
            "thumbnails: cannot create the cache directory {}",
            dir.display()
        )
    })?;
    Ok(dir)
}

/// Lets the webview asset protocol serve the cache directory.
///
/// `tauri.conf.json` already scopes `$LOCALDATA/Hangline/**`, but `$LOCALDATA` is resolved against
/// the OS known folder, not the process environment: a relocated data dir (tests, portable runs)
/// would make every thumb URL fail. Registering the real directory explicitly is cheap and keeps
/// the card's `thumb → original → tile` fallback from silently always taking the original.
pub fn allow_asset_scope(app: &AppHandle) {
    let Ok(dir) = cache_dir() else {
        return;
    };
    match app.asset_protocol_scope().allow_directory(&dir, true) {
        Ok(()) => log::debug!("thumbnails: asset protocol allows {}", dir.display()),
        Err(error) => log::warn!(
            "thumbnails: cannot allow the asset protocol for {}: {error}",
            dir.display()
        ),
    }
}

/// Whether `source` is a recording (decoded through the WinRT first-frame helper, not `image`).
fn is_recording(source: &Path) -> bool {
    source
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
        .is_some_and(|extension| RECORDING_EXTENSIONS.contains(&extension.as_str()))
}

/// Item ids are opaque 32-hex strings; anything else (tampered state) is rejected.
fn is_valid_id(id: &str) -> bool {
    id.len() == 32
        && id
            .chars()
            .all(|character| character.is_ascii_hexdigit() && !character.is_ascii_uppercase())
}

/// `<cache>/<id>.png`, validated against path traversal and tampered ids.
fn cached_path(cache: &Path, id: &str) -> Option<PathBuf> {
    if !is_valid_id(id) {
        log::warn!("thumbnails: refusing invalid item id {id:?}");
        return None;
    }
    let path = cache.join(format!("{id}.png"));
    if !path.starts_with(cache) {
        log::warn!("thumbnails: refusing a cache path outside the cache dir");
        return None;
    }
    Some(path)
}

/// Freshness stamp of `path`, or `None` when it is missing/unreadable.
fn stamp(path: &Path) -> Option<FileStamp> {
    let metadata = fs::metadata(path).ok()?;
    Some(FileStamp {
        modified: metadata.modified().ok(),
    })
}

/// Pure freshness predicate: `true` means the cached thumbnail must be (re)generated.
///
/// - no thumb path/frame (or the file disappeared) → stale;
/// - thumb mtime older than the source → stale;
/// - equal mtime is treated as stale on purpose: a filesystem with coarse timestamps cannot prove
///   which write came last, and rebuilding a 432 px preview is cheaper than showing a pre-edit
///   (e.g. pre-redaction) image. This is the documented semantics for the mtime tiebreaker.
pub(crate) fn needs_thumb(
    thumb: Option<&Path>,
    source: FileStamp,
    thumb_stamp: Option<FileStamp>,
) -> bool {
    let Some(thumb_path) = thumb else {
        return true;
    };
    if thumb_path.as_os_str().is_empty() {
        return true;
    }
    let Some(thumb_stamp) = thumb_stamp else {
        return true;
    };
    match (thumb_stamp.modified, source.modified) {
        (Some(thumb_time), Some(source_time)) => thumb_time <= source_time,
        // A filesystem without mtime support cannot prove freshness: regenerate.
        _ => true,
    }
}

/// Whether the cached preview at `path` is a non-empty, fully decodable image.
///
/// A byte/header check is not enough: a truncated cache file can still carry a valid PNG signature
/// and IHDR, so only a decode proves the preview is usable. The preview is our own ≤432 px PNG, so
/// a full decode costs well under a millisecond and is only paid on the "fresh by mtime" path.
fn preview_decodes(path: &Path) -> bool {
    match image::open(path) {
        Ok(image) => image.width() > 0 && image.height() > 0,
        Err(_) => false,
    }
}

/// Full rebuild decision: the mtime freshness predicate **plus** a content check.
///
/// A preview can be fresh by mtime and still be unusable (a zero-byte file from a crashed write, a
/// truncated disk write, a tampered cache). Returning such a path makes the card fall back to the
/// original forever, because nothing ever heals the cache file. The decode check therefore runs
/// only when [`needs_thumb`] already says "fresh", so the common missing/stale path pays nothing.
///
/// This is the single decision point for both the generator ([`thumb_for`]) and the startup backfill
/// ([`refresh_with_cache`]); the only side effect is the warn line of a self-heal.
pub(crate) fn needs_rebuild(
    thumb: Option<&Path>,
    source_stamp: FileStamp,
    thumb_stamp: Option<FileStamp>,
) -> bool {
    if needs_thumb(thumb, source_stamp, thumb_stamp) {
        return true;
    }
    let Some(path) = thumb else {
        return true;
    };
    if preview_decodes(path) {
        return false;
    }
    log::warn!(
        "thumbnails: cached preview {} is fresh but not a decodable image; rebuilding it from the \
         source",
        path.display()
    );
    true
}

/// Encodes `image` as the cache PNG for `id`, atomically (temp file + rename).
///
/// `image` is a decoded source image; small sources are stored as-is instead of being upscaled.
fn save_thumb_image(image: &image::DynamicImage, cache: &Path, id: &str) -> Option<PathBuf> {
    let target = cached_path(cache, id)?;
    if let Err(error) = fs::create_dir_all(cache) {
        log::warn!("thumbnails: cannot create {}: {error}", cache.display());
        return None;
    }
    let thumb = if image.width() <= THUMB_MAX_EDGE && image.height() <= THUMB_MAX_EDGE {
        // No upscaling: a small source is copied (decoded/re-encoded PNG) instead of resized.
        image.clone()
    } else {
        image.thumbnail(THUMB_MAX_EDGE, THUMB_MAX_EDGE)
    };
    let tmp = cache.join(format!("{id}.png.tmp"));
    // The temp name ends in `.png.tmp`, so the PNG format must be explicit (the extension alone
    // would make `DynamicImage::save` guess an unsupported format).
    if let Err(error) = thumb.save_with_format(&tmp, image::ImageFormat::Png) {
        log::warn!(
            "thumbnails: cannot write {}: {error}; using the tile placeholder",
            tmp.display()
        );
        let _ = fs::remove_file(&tmp);
        return None;
    }
    if let Err(error) = fs::rename(&tmp, &target) {
        log::warn!(
            "thumbnails: cannot move {} into place: {error}",
            tmp.display()
        );
        let _ = fs::remove_file(&tmp);
        return None;
    }
    Some(target)
}

/// Generates (or reuses) `<cache>/<id>.png` for `source`; `None` means "no preview".
///
/// The cache directory is explicit so unit tests never touch `%LOCALAPPDATA%`.
pub(crate) fn thumb_for(source: &Path, cache: &Path, id: &str) -> Option<PathBuf> {
    if !source.is_file() {
        return None;
    }
    if is_recording(source) {
        return None;
    }
    let target = cached_path(cache, id)?;
    let Some(source_stamp) = stamp(source) else {
        return None;
    };
    let thumb_stamp = stamp(&target);
    if !needs_rebuild(Some(&target), source_stamp, thumb_stamp) {
        return Some(target);
    }
    let image = match image::open(source) {
        Ok(image) => image,
        Err(error) => {
            log::debug!(
                "thumbnails: cannot decode {}: {error}; using the tile placeholder",
                source.display()
            );
            return None;
        }
    };
    if image.width() == 0 || image.height() == 0 {
        return None;
    }
    save_thumb_image(&image, cache, id)
}

/// Freshness-checked thumbnail refresh in an explicit cache dir; `true` when the serialized item
/// changed **or** the cached preview was rebuilt in place (a rebuilt preview keeps the same path,
/// so the item fields may be identical). Used by the startup backfill for every non-recording item:
/// the common fresh case is only two metadata reads.
pub(crate) fn refresh_with_cache(item: &mut Item, cache: &Path) -> bool {
    if item.kind == ItemKind::Recording {
        return refresh_recording_with_cache(item, cache);
    }
    let Some(source_stamp) = stamp(&item.path) else {
        // The source vanished between load and backfill; the watcher/prune path owns removal.
        return false;
    };
    let Some(target) = cached_path(cache, &item.id) else {
        return false;
    };
    let needs = needs_rebuild(item.thumb_path.as_deref(), source_stamp, stamp(&target));
    let dims_unknown = item.width == 0 || item.height == 0;
    if !needs && !dims_unknown {
        return false;
    }
    let mut changed = false;
    let mut refreshed = false;
    if needs {
        if let Some(path) = thumb_for(&item.path, cache, &item.id) {
            refreshed = true;
            if item.thumb_path.as_deref() != Some(path.as_path()) {
                item.thumb_path = Some(path);
                changed = true;
            }
        }
    }
    // Width/height repair: decode the header only when the item has no usable dimensions.
    if item.width == 0 || item.height == 0 {
        if let Ok((width, height)) = image::image_dimensions(&item.path) {
            if width > 0 && height > 0 {
                item.width = width;
                item.height = height;
                changed = true;
            }
        }
    }
    // A stale preview rebuilt under the same path does not change the serialized item, but the
    // backfill still needs to know the cache was healed.
    changed || refreshed
}

/// Ensures a thumbnail for `source` in the real cache dir and returns it with decoded dimensions.
pub fn ensure_thumb(source: &Path, id: &str) -> Option<Thumb> {
    let cache = match cache_dir() {
        Ok(cache) => cache,
        Err(error) => {
            log::warn!("thumbnails: cache directory unavailable: {error:#}");
            return None;
        }
    };
    let path = thumb_for(source, &cache, id)?;
    let (width, height) = image::image_dimensions(source).unwrap_or((0, 0));
    Some(Thumb {
        path,
        width,
        height,
    })
}

/// Builds the thumbnail from an already-decoded image (capture path: the PNG was just written).
///
/// Avoids re-decoding the fresh 4K screenshot before the overlay cleanup / reveal / flight can
/// continue; the write itself is still atomic.
fn thumb_from_image_in(image: &RgbaImage, cache: &Path, id: &str) -> Option<PathBuf> {
    if image.width() == 0 || image.height() == 0 {
        return None;
    }
    save_thumb_image(&image::DynamicImage::ImageRgba8(image.clone()), cache, id)
}

/// Public form of [`thumb_from_image_in`] using the real cache dir.
pub fn thumb_from_image(image: &RgbaImage, id: &str) -> Option<PathBuf> {
    let cache = match cache_dir() {
        Ok(cache) => cache,
        Err(error) => {
            log::warn!("thumbnails: cache directory unavailable: {error:#}");
            return None;
        }
    };
    thumb_from_image_in(image, &cache, id)
}

/// Rebuilds the 432 px preview for `id` from an already-decoded **rewritten** image, unconditionally.
///
/// The P3 annotate write-back replaces the source file in place and must show the new pixels
/// immediately. The normal freshness check cannot prove ordering when the source and its preview
/// are written within the same filesystem timestamp tick, so this entry point always regenerates
/// the cache file (atomically: `<id>.png.tmp` + rename). `None` means "no preview" (recording,
/// empty image, invalid id or a cache write failure) and leaves any previous preview untouched.
/// The caller passes the real cache directory so the write-back can be unit-tested off
/// `%LOCALAPPDATA%`.
pub(crate) fn refresh_from_image(
    cache: &Path,
    source: &Path,
    id: &str,
    image: &RgbaImage,
) -> Option<PathBuf> {
    if is_recording(source) {
        return None;
    }
    if image.width() == 0 || image.height() == 0 {
        return None;
    }
    if let Err(error) = fs::create_dir_all(cache) {
        log::warn!("thumbnails: cannot create {}: {error}", cache.display());
        return None;
    }
    save_thumb_image(&image::DynamicImage::ImageRgba8(image.clone()), cache, id)
}

/// Attaches a generated thumbnail to `item` (and repairs 0×0 dimensions when the source decodes).
///
/// Called by every non-capture ingest path **before** `AppState::add_item`, so the first `state`
/// broadcast and its persisted `state.json` already carry `thumbPath`; capture reuses the decoded
/// crop through [`attach_from_image`]. Recordings go through the WinRT first-frame helper
/// (`crate::video`) and keep `thumb_path = None` when the media stack cannot decode them (tile +
/// play-badge fallback).
pub fn attach(item: &mut Item) {
    if item.kind == ItemKind::Recording {
        attach_recording(item);
        return;
    }
    if let Some(thumb) = ensure_thumb(&item.path, &item.id) {
        item.thumb_path = Some(thumb.path);
        if item.width == 0 || item.height == 0 {
            item.width = thumb.width;
            item.height = thumb.height;
        }
    }
}

/// Recording ingest path: decode the first frame, write the 432 px preview, repair dimensions.
fn attach_recording(item: &mut Item) {
    let info = match crate::video::first_frame(&item.path, THUMB_MAX_EDGE) {
        Ok(info) => info,
        Err(error) => {
            log::info!(
                "thumbnails: no first frame for {}: {error:#}",
                item.path.display()
            );
            return;
        }
    };
    let cache = match cache_dir() {
        Ok(cache) => cache,
        Err(error) => {
            log::warn!("thumbnails: cache directory unavailable: {error:#}");
            return;
        }
    };
    if !attach_recording_preview(item, &cache, &info) {
        log::warn!(
            "thumbnails: cannot write the recording preview for {}",
            item.path.display()
        );
    }
}

/// Applies one decoded first frame to `item` in an explicit cache dir.
///
/// The item keeps the video's natural pixel dimensions (`info.width`/`info.height`) so the card
/// layout preserves the recording's aspect ratio; the preview itself is the 432 px frame written
/// atomically by [`save_thumb_image`]. Returns whether any item field changed (the backfill's
/// state-change signal).
pub(crate) fn attach_recording_preview(item: &mut Item, cache: &Path, info: &crate::video::VideoInfo) -> bool {
    let (width, height) = if info.width > 0 && info.height > 0 {
        (info.width, info.height)
    } else {
        (info.image.width(), info.image.height())
    };
    let mut changed = false;
    if width > 0 && height > 0 && (item.width != width || item.height != height) {
        item.width = width;
        item.height = height;
        changed = true;
    }
    let image = image::DynamicImage::ImageRgba8(info.image.clone());
    if let Some(path) = save_thumb_image(&image, cache, &item.id) {
        if item.thumb_path.as_deref() != Some(path.as_path()) {
            item.thumb_path = Some(path);
            changed = true;
        }
    }
    changed
}

/// Whether a recording still needs a decode: unknown dimensions or a missing preview file.
fn recording_needs_preview(item: &Item) -> bool {
    let dimensions_unknown = item.width == 0 || item.height == 0;
    let preview_missing = item
        .thumb_path
        .as_deref()
        .is_none_or(|path| !path.is_file());
    dimensions_unknown || preview_missing
}

/// Startup-backfill variant of [`attach_recording`] in an explicit cache dir.
fn refresh_recording_with_cache(item: &mut Item, cache: &Path) -> bool {
    if !recording_needs_preview(item) {
        return false;
    }
    let info = match crate::video::first_frame(&item.path, THUMB_MAX_EDGE) {
        Ok(info) => info,
        Err(error) => {
            log::info!(
                "thumbnails: backfill has no first frame for {}: {error:#}",
                item.path.display()
            );
            return false;
        }
    };
    attach_recording_preview(item, cache, &info)
}

/// Capture-path variant of [`attach`]: the decoded crop is reused instead of re-reading the PNG.
pub fn attach_from_image(item: &mut Item, image: &RgbaImage) {
    if let Some(path) = thumb_from_image(image, &item.id) {
        item.thumb_path = Some(path);
        if item.width == 0 || item.height == 0 {
            item.width = image.width();
            item.height = image.height();
        }
    }
}

/// Removes the cached preview for `id` (item removal/prune); missing files are fine.
pub fn remove_for_id(id: &str) {
    // Validate before touching the cache dir: removal callers may hold an arbitrary id and a unit
    // test must never create `%LOCALAPPDATA%\Hangline\Thumbnails` as a side effect.
    if !is_valid_id(id) {
        log::debug!("thumbnails: not removing an invalid id {id:?}");
        return;
    }
    let Ok(cache) = cache_dir() else {
        return;
    };
    let Some(path) = cached_path(&cache, id) else {
        return;
    };
    match fs::remove_file(&path) {
        Ok(()) => log::debug!("thumbnails: removed {}", path.display()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => log::warn!("thumbnails: cannot remove {}: {error}", path.display()),
    }
}

/// Stale temp files younger than this are assumed to be an in-flight write from another process.
const STALE_TEMP_AGE: std::time::Duration = std::time::Duration::from_secs(60 * 60);

/// Deletes cache files that no longer belong to a hung item plus every leftover `.png.tmp` older
/// than [`STALE_TEMP_AGE`].
///
/// Runs once at startup (after the state load), so removing items never leaks thumbnails and a
/// crash during a write never leaves old temp files behind. A fresh temp file is left alone: another
/// process (or a very recent crash) may still be writing it.
pub(crate) fn sweep_orphans(cache: &Path, valid_ids: &HashSet<String>) -> (usize, usize) {
    let Ok(entries) = fs::read_dir(cache) else {
        return (0, 0);
    };
    let mut removed_files = 0usize;
    let mut removed_temps = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name.ends_with(".png.tmp") {
            let stale = entry
                .metadata()
                .ok()
                .and_then(|metadata| metadata.modified().ok())
                .and_then(|modified| modified.elapsed().ok())
                .is_some_and(|age| age > STALE_TEMP_AGE);
            if stale && fs::remove_file(&path).is_ok() {
                removed_temps += 1;
            }
            continue;
        }
        let Some(id) = name.strip_suffix(".png") else {
            continue;
        };
        if is_valid_id(id) && valid_ids.contains(id) {
            continue;
        }
        if fs::remove_file(&path).is_ok() {
            removed_files += 1;
        }
    }
    if removed_files > 0 || removed_temps > 0 {
        log::info!(
            "thumbnails: swept {removed_files} orphan(s) and {removed_temps} stale temp file(s)"
        );
    }
    (removed_files, removed_temps)
}

/// Startup backfill: fills missing/stale thumbnails for the loaded items on a named background
/// thread, sweeps orphans, then persists + emits one state snapshot when anything changed.
pub fn backfill(app: &AppHandle) {
    let handle = app.clone();
    let spawned = std::thread::Builder::new()
        .name("hangline-thumbnails".to_string())
        .spawn(move || {
            let state = handle.state::<AppState>();
            let snapshot = state.snapshot();
            let cache = match cache_dir() {
                Ok(cache) => cache,
                Err(error) => {
                    log::warn!("thumbnails: backfill has no cache dir: {error:#}");
                    return;
                }
            };
            let valid_ids: HashSet<String> =
                snapshot.items.iter().map(|item| item.id.clone()).collect();
            sweep_orphans(&cache, &valid_ids);
            let mut updates = Vec::new();
            for mut item in snapshot.items {
                // Removal race: generating earlier previews takes time, so the item may have left
                // the line meanwhile. Re-check immediately before writing its preview...
                if state.item(&item.id).is_none() {
                    continue;
                }
                if refresh_with_cache(&mut item, &cache) {
                    // ...and again after the decode/write: if the item left in between, undo the
                    // file we just wrote and never apply the update.
                    if state.item(&item.id).is_some() {
                        updates.push(item);
                    } else {
                        remove_for_id(&item.id);
                    }
                }
            }
            if updates.is_empty() {
                log::info!("thumbnails: backfill found nothing to do");
                return;
            }
            // A stale preview rebuilt under the same path does not change the serialized item, so
            // this line is the evidence that the cache itself was healed on startup. Known
            // limitation (P2-9): a byte-only heal keeps the same `thumbPath` URL, so a WebView that
            // already cached those bytes may show the old preview until the next reload/restart.
            log::info!(
                "thumbnails: backfill refreshed {} preview(s)",
                updates.len()
            );
            match state.apply_thumb_updates(&updates) {
                Ok(true) => {
                    log::info!("thumbnails: backfilled {} thumbnail(s)", updates.len());
                    if app_state::emit_state(&handle).is_none() {
                        log::warn!("thumbnails: backfill could not emit the state snapshot");
                    }
                }
                Ok(false) => log::info!("thumbnails: backfill made no state change"),
                Err(error) => log::warn!("thumbnails: backfill could not persist: {error:#}"),
            }
        });
    if let Err(error) = spawned {
        log::warn!("thumbnails: cannot spawn the backfill thread: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;

    /// Scratch directory under `target/` (git-ignored); never touches `%LOCALAPPDATA%`.
    fn test_dir(tag: &str) -> PathBuf {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("thumb-tests")
            .join(format!("{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create the thumbnail test dir");
        dir
    }

    fn source(dir: &Path, name: &str, width: u32, height: u32) -> PathBuf {
        let path = dir.join(name);
        image::RgbaImage::from_pixel(width, height, image::Rgba([10, 20, 30, 255]))
            .save(&path)
            .expect("write the test source");
        path
    }

    const TEST_ID: &str = "0123456789abcdef0123456789abcdef";

    fn test_item(id: &str, path: &Path) -> Item {
        Item {
            id: id.to_string(),
            path: path.to_path_buf(),
            thumb_path: None,
            kind: ItemKind::Capture,
            hash: 1,
            width: 10,
            height: 20,
            created_at: 0,
            added_at: 0,
            updated_at: 0,
        }
    }

    #[test]
    fn thumbnails_fit_the_long_edge_and_are_idempotent() {
        let dir = test_dir("fit");
        let cache = dir.join("cache");
        let src = source(&dir, "source.png", 4000, 3000);
        let first_path = thumb_for(&src, &cache, TEST_ID).expect("first thumb");
        let decoded = image::open(&first_path).expect("open the thumb");
        assert_eq!(decoded.width().max(decoded.height()), THUMB_MAX_EDGE);
        // Aspect preserved: the 4:3 source stays 4:3.
        assert_eq!(decoded.width(), 432);
        assert_eq!(decoded.height(), 324);

        let first_mtime = fs::metadata(&first_path)
            .unwrap()
            .modified()
            .expect("thumb mtime");
        let second_path = thumb_for(&src, &cache, TEST_ID).expect("reused thumb");
        assert_eq!(first_path, second_path);
        let second_mtime = fs::metadata(&second_path)
            .unwrap()
            .modified()
            .expect("thumb mtime");
        assert_eq!(first_mtime, second_mtime, "a fresh cache file is reused");
    }

    #[test]
    fn small_sources_are_copied_without_upscaling() {
        let dir = test_dir("small");
        let cache = dir.join("cache");
        let src = source(&dir, "small.png", 320, 200);
        let thumb_path = thumb_for(&src, &cache, "0123456789abcdef0123456789abcdef")
            .expect("small thumb");
        let decoded = image::open(&thumb_path).expect("open the small thumb");
        assert_eq!((decoded.width(), decoded.height()), (320, 200));
    }

    #[test]
    fn corrupt_sources_return_none() {
        let dir = test_dir("bad");
        let cache = dir.join("cache");
        let src = dir.join("bad.png");
        fs::write(&src, b"not a png").expect("write the corrupt file");
        assert!(thumb_for(&src, &cache, TEST_ID).is_none());
    }

    /// Cache files with a `.tmp` suffix (the atomic writer's only residue on failure).
    fn temp_files(cache: &Path) -> Vec<String> {
        if !cache.exists() {
            return Vec::new();
        }
        fs::read_dir(cache)
            .expect("read the cache dir")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".tmp"))
            .collect()
    }

    #[test]
    fn preview_decode_check_rejects_zero_byte_text_and_truncated_files() {
        let dir = test_dir("decode-check");
        let cache = dir.join("cache");
        let valid = source(&dir, "valid.png", 320, 200);
        assert!(preview_decodes(&valid), "a real PNG decodes");
        let empty = dir.join("empty.png");
        fs::write(&empty, b"").expect("write an empty file");
        assert!(!preview_decodes(&empty), "a zero-byte preview is not a thumbnail");
        let text = dir.join("text.png");
        fs::write(&text, b"not a png at all").expect("write a text file");
        assert!(!preview_decodes(&text), "a non-image cache file is rejected");
        // Truncated after a valid signature/header: the header alone would look decodable.
        let bytes = fs::read(&valid).expect("read the valid png");
        let truncated = dir.join("truncated.png");
        fs::write(&truncated, &bytes[..bytes.len() / 2]).expect("write a truncated png");
        assert!(
            image::image_dimensions(&truncated).is_ok(),
            "the truncated file still has a valid header (the case a header-only check misses)"
        );
        assert!(!preview_decodes(&truncated), "a truncated preview is not readable");
        assert!(!cache.exists());
    }

    #[test]
    fn a_fresh_but_corrupt_preview_is_rebuilt_from_the_source() {
        let dir = test_dir("fresh-corrupt");
        let cache = dir.join("cache");
        let src = source(&dir, "source.png", 800, 600);
        let thumb_path = thumb_for(&src, &cache, TEST_ID).expect("first thumb");
        let good_len = fs::metadata(&thumb_path).expect("thumb metadata").len();
        assert!(good_len > 0);
        // Zero-byte cache file with an mtime *newer* than the source: the old mtime predicate alone
        // called this fresh and returned the broken path forever (P2-7 backlog self-heal).
        fs::write(&thumb_path, b"").expect("corrupt the cached preview");
        let cache_mtime = fs::metadata(&thumb_path).unwrap().modified().unwrap();
        let source_mtime = fs::metadata(&src).unwrap().modified().unwrap();
        assert!(cache_mtime > source_mtime, "the corrupt preview must look fresh");

        let rebuilt = thumb_for(&src, &cache, TEST_ID).expect("the corrupt preview is rebuilt");
        assert_eq!(rebuilt, thumb_path, "the preview is healed in place");
        assert_eq!(
            fs::metadata(&thumb_path).unwrap().len(),
            good_len,
            "the rebuilt preview has real bytes again"
        );
        let decoded = image::open(&thumb_path).expect("the rebuilt preview decodes");
        assert_eq!(decoded.width().max(decoded.height()), THUMB_MAX_EDGE);
        assert!(temp_files(&cache).is_empty(), "no temp residue after the rebuild");
    }

    #[test]
    fn a_fresh_valid_preview_is_never_rebuilt() {
        let dir = test_dir("fresh-valid");
        let cache = dir.join("cache");
        let src = source(&dir, "source.png", 800, 600);
        let thumb_path = thumb_for(&src, &cache, TEST_ID).expect("first thumb");
        let before = fs::metadata(&thumb_path).unwrap();
        let bytes_before = fs::read(&thumb_path).expect("read the first preview");
        let reused = thumb_for(&src, &cache, TEST_ID).expect("reused thumb");
        assert_eq!(reused, thumb_path);
        let after = fs::metadata(&thumb_path).unwrap();
        assert_eq!(
            before.modified().unwrap(),
            after.modified().unwrap(),
            "a valid fresh preview is untouched (same mtime)"
        );
        assert_eq!(
            fs::read(&thumb_path).expect("read the preview"),
            bytes_before,
            "a valid fresh preview is byte-identical"
        );
        assert!(temp_files(&cache).is_empty());
    }

    #[test]
    fn a_failed_rebuild_returns_none_and_leaves_no_temp_residue() {
        let dir = test_dir("rebuild-fails");
        let cache = dir.join("cache");
        let src = source(&dir, "source.png", 800, 600);
        let thumb_path = thumb_for(&src, &cache, TEST_ID).expect("first thumb");
        // Both the cache and the source are unreadable: the self-heal attempt must fail cleanly.
        fs::write(&thumb_path, b"").expect("corrupt the cached preview");
        fs::write(&src, b"the source is gone too").expect("corrupt the source");
        assert!(
            thumb_for(&src, &cache, TEST_ID).is_none(),
            "an unreadable source yields no preview"
        );
        assert!(temp_files(&cache).is_empty(), "no temp residue after the failure");
        assert!(
            thumb_path.exists(),
            "the bad cache file is left to the next attempt, never a user file"
        );
    }

    #[test]
    fn the_startup_refresh_heals_a_fresh_but_corrupt_preview() {
        let dir = test_dir("refresh-corrupt");
        let cache = dir.join("cache");
        let src = source(&dir, "source.png", 800, 600);
        let thumb_path = thumb_for(&src, &cache, TEST_ID).expect("first thumb");
        let mut item = test_item(TEST_ID, &src);
        item.thumb_path = Some(thumb_path.clone());
        fs::write(&thumb_path, b"").expect("corrupt the cached preview");
        // The cache mtime is newer than the source: only the content check can see the damage.
        let cache_mtime = fs::metadata(&thumb_path).unwrap().modified().unwrap();
        let source_mtime = fs::metadata(&src).unwrap().modified().unwrap();
        assert!(
            cache_mtime > source_mtime,
            "the corrupt preview must look fresh by mtime"
        );
        assert!(
            refresh_with_cache(&mut item, &cache),
            "the backfill reports the healed preview"
        );
        assert!(
            image::open(&thumb_path).is_ok(),
            "the startup backfill rebuilt the corrupt preview"
        );
        assert!(temp_files(&cache).is_empty());
    }

    #[test]
    fn recording_sources_return_none() {
        let dir = test_dir("recording");
        let cache = dir.join("cache");
        let src = dir.join("clip.mp4");
        fs::write(&src, b"not really a video").expect("write the recording placeholder");
        assert!(thumb_for(&src, &cache, "0123456789abcdef0123456789abcdef").is_none());
        assert!(!cache.exists(), "a recording must not create the cache dir");
    }

    #[test]
    fn missing_sources_return_none() {
        let dir = test_dir("missing");
        let cache = dir.join("cache");
        assert!(thumb_for(
            &dir.join("nope.png"),
            &cache,
            "0123456789abcdef0123456789abcdef"
        )
        .is_none());
    }

    #[test]
    fn stale_sources_regenerate_and_clean_the_temp_file() {
        let dir = test_dir("stale");
        let cache = dir.join("cache");
        let src = source(&dir, "source.png", 800, 600);
        let thumb_path = thumb_for(&src, &cache, "0123456789abcdef0123456789abcdef")
            .expect("first thumb");
        let first_mtime = fs::metadata(&thumb_path).unwrap().modified().unwrap();

        // Rewrite the source (new mtime/content) and ask again: the cache must be replaced.
        std::thread::sleep(std::time::Duration::from_millis(20));
        image::RgbaImage::from_pixel(800, 600, image::Rgba([200, 100, 50, 255]))
            .save(&src)
            .expect("rewrite the source");
        let id = "0123456789abcdef0123456789abcdef";
        let regenerated = thumb_for(&src, &cache, id).expect("regenerated thumb");
        assert_eq!(regenerated, thumb_path);
        let second_mtime = fs::metadata(&regenerated).unwrap().modified().unwrap();
        assert!(second_mtime > first_mtime);
        assert!(!cache.join(format!("{id}.png.tmp")).exists(), "no temp file is left");
    }

    #[test]
    fn freshness_predicate_covers_missing_stale_and_equal_mtime() {
        let early = FileStamp {
            modified: Some(SystemTime::UNIX_EPOCH),
        };
        let later = FileStamp {
            modified: Some(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(5)),
        };
        let thumb = Path::new("cache.png");
        // Missing thumb path or missing cache metadata.
        assert!(needs_thumb(None, early, None));
        assert!(needs_thumb(Some(thumb), early, None));
        // Older thumb mtime.
        assert!(needs_thumb(Some(thumb), later, Some(early)));
        // Fresh thumb (written after the source).
        assert!(!needs_thumb(Some(thumb), early, Some(later)));
        // Equal mtime is ambiguous on coarse filesystems: rebuild rather than risk a stale preview.
        assert!(needs_thumb(Some(thumb), early, Some(early)));
    }

    #[test]
    fn refresh_heals_a_stale_thumb_in_place() {
        let dir = test_dir("refresh");
        let cache = dir.join("cache");
        let src = source(&dir, "source.png", 100, 80);
        let id = "0123456789abcdef0123456789abcdef";
        let mut item = test_item(id, &src);
        assert!(refresh_with_cache(&mut item, &cache), "first refresh attaches");
        let first_thumb = item.thumb_path.clone().expect("thumb attached");
        // Idempotent: a fresh thumb means no work.
        assert!(!refresh_with_cache(&mut item, &cache));

        // Overwrite the source in place (new mtime) while the app was closed, then restart-like
        // refresh: the stale preview must be regenerated even though the file exists.
        std::thread::sleep(std::time::Duration::from_millis(20));
        image::RgbaImage::from_pixel(100, 80, image::Rgba([250, 10, 10, 255]))
            .save(&src)
            .expect("overwrite the source");
        assert!(
            refresh_with_cache(&mut item, &cache),
            "stale thumb is regenerated"
        );
        assert_eq!(item.thumb_path.as_ref(), Some(&first_thumb));
        let decoded = image::open(&first_thumb).unwrap().to_rgba8();
        let pixel = decoded.get_pixel(0, 0);
        assert_eq!(pixel.0, [250, 10, 10, 255], "the thumb now shows the new source");
        assert!(!refresh_with_cache(&mut item, &cache));
    }

    #[test]
    fn refresh_repairs_unknown_dimensions() {
        let dir = test_dir("dims");
        let cache = dir.join("cache");
        let src = source(&dir, "source.png", 640, 480);
        let id = "0123456789abcdef0123456789abcdef";
        let mut item = test_item(id, &src);
        item.width = 0;
        item.height = 0;
        // Preseed a fresh thumb so only the dimension repair is missing.
        thumb_for(&src, &cache, id).expect("preseed thumb");
        item.thumb_path = Some(cache.join(format!("{id}.png")));
        assert!(refresh_with_cache(&mut item, &cache));
        assert_eq!((item.width, item.height), (640, 480));
    }

    #[test]
    fn thumb_paths_reject_tampered_ids() {
        let dir = test_dir("paths");
        let cache = dir.join("cache");
        let src = source(&dir, "source.png", 100, 100);
        assert!(thumb_for(&src, &cache, "../escape").is_none());
        assert!(thumb_for(&src, &cache, "XYZ").is_none());
        assert!(thumb_for(&src, &cache, "0123456789ABCDEF0123456789ABCDEF").is_none());
        assert!(!cache.exists(), "no cache file is created for an invalid id");
    }

    #[test]
    fn orphan_sweep_removes_unknown_ids_and_old_temp_files() {
        let dir = test_dir("sweep");
        let cache = dir.join("cache");
        fs::create_dir_all(&cache).expect("cache dir");
        let kept = "0123456789abcdef0123456789abcdef".to_string();
        fs::write(cache.join(format!("{kept}.png")), b"kept").expect("kept thumb");
        fs::write(
            cache.join("fedcba9876543210fedcba9876543210.png"),
            b"orphan",
        )
        .expect("orphan thumb");
        fs::write(cache.join("tampered-id.png"), b"bad name").expect("bad name");
        // One old temp file (a crash residue) is swept; a fresh one may be an in-flight write.
        let old_temp = cache.join(format!("{kept}.png.tmp"));
        fs::write(&old_temp, b"old tmp").expect("old temp");
        fs::File::options()
            .write(true)
            .open(&old_temp)
            .expect("open old temp")
            .set_modified(SystemTime::now() - std::time::Duration::from_secs(2 * 60 * 60))
            .expect("age the old temp");
        let fresh_temp = cache.join("fedcba9876543210fedcba9876543210.png.tmp");
        fs::write(&fresh_temp, b"fresh tmp").expect("fresh temp");
        let mut valid = HashSet::new();
        valid.insert(kept.clone());
        let (orphans, temps) = sweep_orphans(&cache, &valid);
        assert_eq!((orphans, temps), (2, 1));
        assert!(cache.join(format!("{kept}.png")).exists());
        assert!(!cache.join("fedcba9876543210fedcba9876543210.png").exists());
        assert!(!cache.join("tampered-id.png").exists());
        assert!(!old_temp.exists());
        assert!(fresh_temp.exists(), "a fresh temp file may be an in-flight write");
    }

    #[test]
    fn capture_image_thumb_uses_the_decoded_crop() {
        let dir = test_dir("capture");
        let cache = dir.join("cache");
        let image = RgbaImage::from_pixel(3840, 2160, image::Rgba([1, 2, 3, 255]));
        let path = thumb_from_image_in(&image, &cache, TEST_ID)
            .expect("in-memory thumb");
        let decoded = image::open(path).expect("open the thumb");
        assert_eq!(decoded.width().max(decoded.height()), THUMB_MAX_EDGE);
        assert_eq!((decoded.width(), decoded.height()), (432, 243));
    }

    #[test]
    fn forced_refresh_overwrites_a_fresh_thumb_with_the_new_pixels() {
        // P3 save path: the source is replaced in place and the preview must show the new bytes even
        // though the old preview is still "fresh" by mtime.
        let dir = test_dir("force-refresh");
        let cache = dir.join("cache");
        let src = source(&dir, "source.png", 800, 600);
        let mut item = test_item(TEST_ID, &src);
        assert!(refresh_with_cache(&mut item, &cache), "initial preview");
        let thumb_path = item.thumb_path.clone().expect("thumb attached");
        let decoded = image::open(&thumb_path).unwrap().to_rgba8();
        assert_eq!(decoded.get_pixel(0, 0).0, [10, 20, 30, 255]);

        let rewritten = RgbaImage::from_pixel(800, 600, image::Rgba([250, 10, 10, 255]));
        let refreshed = refresh_from_image(&cache, &src, TEST_ID, &rewritten)
            .expect("forced refresh replaces the cached preview");
        assert_eq!(refreshed, thumb_path, "the cache path is stable");
        let decoded = image::open(&refreshed).unwrap().to_rgba8();
        assert_eq!(
            decoded.get_pixel(0, 0).0,
            [250, 10, 10, 255],
            "the forced refresh must not reuse the fresh preview"
        );
        assert!(
            !cache.join(format!("{TEST_ID}.png.tmp")).exists(),
            "the atomic write must not leave a temp file"
        );
    }

    #[test]
    fn forced_refresh_fits_432_and_rejects_invalid_inputs() {
        let dir = test_dir("force-refresh-guards");
        let cache = dir.join("cache");
        let src = source(&dir, "source.png", 4000, 3000);
        let image = RgbaImage::from_pixel(4000, 3000, image::Rgba([1, 2, 3, 255]));
        let thumb = refresh_from_image(&cache, &src, TEST_ID, &image).expect("432 preview");
        let decoded = image::open(&thumb).expect("open the thumb");
        assert_eq!((decoded.width(), decoded.height()), (432, 324));

        // Recordings never get a cached preview, a tampered id is refused, and a 0x0 image is a miss.
        let recording = dir.join("clip.mp4");
        fs::write(&recording, b"not really a video").expect("write the recording placeholder");
        assert!(refresh_from_image(&cache, &recording, TEST_ID, &image).is_none());
        assert!(refresh_from_image(&cache, &src, "../escape", &image).is_none());
        assert!(refresh_from_image(&cache, &src, TEST_ID, &RgbaImage::new(0, 0)).is_none());
    }

    /// A synthetic first-frame result (the real one comes from `video::first_frame`).
    fn recording_info(width: u32, height: u32) -> crate::video::VideoInfo {
        crate::video::VideoInfo {
            image: RgbaImage::from_pixel(width, height, image::Rgba([24, 120, 116, 255])),
            width,
            height,
            duration_ms: 3000,
        }
    }

    fn recording_item(id: &str, path: &Path) -> Item {
        let mut item = test_item(id, path);
        item.kind = ItemKind::Recording;
        item.width = 0;
        item.height = 0;
        item
    }

    #[test]
    fn recording_preview_uses_the_frame_and_natural_dimensions() {
        let dir = test_dir("recording-preview");
        let cache = dir.join("cache");
        let source_path = dir.join("clip.mp4");
        fs::write(&source_path, b"not decoded here").expect("write the placeholder recording");
        let mut item = recording_item(TEST_ID, &source_path);
        let info = recording_info(640, 360);

        assert!(
            attach_recording_preview(&mut item, &cache, &info),
            "a decodable first frame must attach a preview"
        );
        let thumb = item.thumb_path.clone().expect("thumbPath is set");
        assert_eq!(thumb, cache.join(format!("{TEST_ID}.png")));
        let decoded = image::open(&thumb).expect("open the recording preview");
        assert_eq!(
            (decoded.width(), decoded.height()),
            (THUMB_MAX_EDGE, 243),
            "the preview is 432 px on its longest edge, 16:9 preserved"
        );
        assert_eq!(
            (item.width, item.height),
            (640, 360),
            "item dimensions are the natural video size, not the scaled preview"
        );
        assert!(
            !cache.join(format!("{TEST_ID}.png.tmp")).exists(),
            "the preview write must not leave a temp file"
        );
    }

    #[test]
    fn recording_preview_failure_keeps_the_play_badge_fallback() {
        // A missing source is rejected before WinRT is touched, so this stays a normal unit test.
        let dir = test_dir("recording-missing");
        let mut item = recording_item(TEST_ID, &dir.join("missing.mp4"));
        attach(&mut item);
        assert_eq!(item.thumb_path, None);
        assert_eq!((item.width, item.height), (0, 0));
    }

    #[test]
    fn recording_refresh_predicate_covers_missing_dimensions_and_preview() {
        let dir = test_dir("recording-predicate");
        let mut item = recording_item(TEST_ID, &dir.join("clip.mp4"));
        assert!(recording_needs_preview(&item), "no dimensions and no thumb");

        item.width = 640;
        item.height = 360;
        assert!(recording_needs_preview(&item), "dimensions known, no thumb");

        let thumb = dir.join(format!("{TEST_ID}.png"));
        fs::write(&thumb, b"thumb").expect("write the thumb placeholder");
        item.thumb_path = Some(thumb.clone());
        assert!(
            !recording_needs_preview(&item),
            "known dimensions and an existing preview need no video decode"
        );

        item.thumb_path = Some(dir.join("gone.png"));
        assert!(recording_needs_preview(&item), "a vanished preview is rebuilt");
    }

    #[test]
    fn recording_backfill_skips_missing_sources_without_creating_the_cache() {
        let dir = test_dir("recording-backfill-missing");
        let cache = dir.join("cache");
        let mut item = recording_item(TEST_ID, &dir.join("missing.mp4"));
        assert!(!refresh_with_cache(&mut item, &cache));
        assert_eq!(item.thumb_path, None);
        assert!(!cache.exists(), "a missing recording must not create the cache");
    }
}
