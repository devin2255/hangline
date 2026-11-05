//! P3 markup editor: the annotate window, its target payload, and the atomic PNG write-back.
//!
//! The editor is a single, borderless, focusable window (`index.html?window=annotate&id=…`) whose
//! size fits the screenshot into 80 % of the target monitor's work area (max 1400×900, min
//! 480×360). The frontend draws the annotations and serializes a full-resolution PNG; this module
//! is the only writer:
//!
//! 1. re-validate the item (it must still be on the line and inside Hangline's screenshot folder —
//!    the frontend's disabled Save button is not a security boundary);
//! 2. decode + size-check the payload, then re-encode a canonical RGBA8 PNG;
//! 3. write it to `<dir>/.hangline-markup-<token>.png.tmp` in the **same directory**, `sync_all`,
//!    and rename over the original (`std::fs::rename` replaces an existing file on Windows);
//! 4. compute the BLAKE3-64 hash of the exact bytes, force-regenerate the 432 px preview, update
//!    width/height/hash/`updated_at`, persist through [`AppState::replace_item`] and broadcast.
//!
//! Every failure removes the temp file and leaves the original untouched. A save failure rate of
//! zero over 100 consecutive replacements is the P3 definition of done (see the `stress` test).

use std::fs;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{bail, Context};
use base64::prelude::*;
use image::codecs::png::PngEncoder;
use image::{ExtendedColorType, ImageEncoder, RgbaImage};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

use crate::app_state::{self, AppState};
use crate::capture;
use crate::dedupe;
use crate::editor_window;
use crate::item::{self, Item};
use crate::settings::Theme;
use crate::thumbnails;

/// Label of the markup editor window (single instance).
pub const LABEL: &str = "annotate";

/// Window title (custom titlebar text comes from the frontend i18n table).
const WINDOW_TITLE: &str = "Hangline Markup";

/// Event asking an already-open editor to switch to another item (P3-4 opening a second card).
pub const TARGET_EVENT: &str = "annotate-target";

/// Event telling the panel whether the editor window is open (P3-4: keep the line from tucking).
pub const STATE_EVENT: &str = "editor-state";

/// Payload of [`STATE_EVENT`].
#[derive(Clone, Copy, Debug, Serialize)]
pub struct EditorStateEvent {
    /// True while the single editor window exists (including while it has unsaved edits).
    pub open: bool,
}

/// Maximum editor content width (design spec §3.7).
pub const MAX_WINDOW_W: f64 = 1400.0;
/// Maximum editor content height (design spec §3.7).
pub const MAX_WINDOW_H: f64 = 900.0;
/// Minimum editor content width (plan P3-3; extreme aspect ratios are clamped, see
/// [`editor_window_size`]).
pub const MIN_WINDOW_W: f64 = 480.0;
/// Minimum editor content height (plan P3-3).
pub const MIN_WINDOW_H: f64 = 360.0;
/// Fraction of the monitor work area the screenshot may occupy (design spec §3.7).
pub const WORK_AREA_FRACTION: f64 = 0.8;
/// Maximum accepted image edge (plan P3-3).
pub const MAX_IMAGE_EDGE: u32 = 12_000;
/// Maximum accepted decoded RGBA size in bytes (plan P3-3).
pub const MAX_DECODED_BYTES: u64 = 128 * 1024 * 1024;
/// Maximum accepted base64 payload size in bytes (plan P3-3).
pub const MAX_PAYLOAD_BYTES: usize = 64 * 1024 * 1024;

/// Prefix of the same-directory write-back temp file.
const TEMP_PREFIX: &str = ".hangline-markup-";

/// Sequence making temp names unique when two saves land in the same millisecond.
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Age after which a crashed write-back temp file is swept at startup (thumbnail sweep parity).
const STALE_TEMP_AGE: Duration = Duration::from_secs(60 * 60);

/// Whether the open editor has unsaved edits. The frontend owns the dirty definition (shapes,
/// crop, a non-empty text entry) and mirrors it through `set_editor_dirty`; `open_annotate` uses
/// this flag to focus an existing editor instead of silently discarding those edits.
static EDITOR_DIRTY: AtomicBool = AtomicBool::new(false);

/// Whether the editor page registered its `annotate-target` listener (ready handshake).
static EDITOR_READY: AtomicBool = AtomicBool::new(false);

/// Last item id the editor should show; replayed by the ready handshake when the page was still
/// loading when a second `open_annotate` arrived.
static EDITOR_TARGET: Mutex<Option<String>> = Mutex::new(None);

/// What the editor page needs to render one item (design spec §4.4 / plan P3-3).
///
/// `owned` is `true` only when the file is inside `%LOCALAPPDATA%\Hangline\Screenshots`; watched
/// folders open read-only (Save is disabled in the UI and rejected again by `save_annotation`).
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditorTarget {
    /// The item currently on the line.
    pub item: Item,
    /// Whether Hangline may rewrite this file in place.
    pub owned: bool,
    /// Raw `settings.theme` (`system`/`light`/`dark`); the page resolves `system` itself.
    pub theme: String,
}

/// The URL/query of the editor page (unit-tested; ids are opaque 32-hex strings).
///
/// `lang` is the Rust-resolved UI language (`en`/`zh-Hans`). Carrying it in the URL lets the page
/// apply the right i18n table before the first paint instead of flashing English until the async
/// `get_app_state` snapshot arrives.
pub(crate) fn annotate_url(id: &str, theme: Theme, lang: &str) -> String {
    format!(
        "index.html?window=annotate&id={id}&theme={}&lang={lang}",
        theme.as_str()
    )
}

/// Content size for one image: fit it into 80 % of the work area, max 1400×900, min 480×360.
///
/// The point is scaled before the minimum clamp, so the **window** preserves the image aspect ratio
/// for all normal screenshots. An extreme ratio (e.g. a 200×20000 strip) hits the 480 px minimum on
/// one axis and the window is no longer proportional — the image is still shown with
/// `object-fit: contain`, so the drawing area keeps its own aspect ratio. The final size is clamped
/// to the work area (a work area smaller than 480×360 wins over the minimum). All values are CSS
/// pixels of the editor page; the caller converts them to physical pixels with the monitor DPI.
pub fn editor_window_size(
    image_width: u32,
    image_height: u32,
    work_width: f64,
    work_height: f64,
) -> (f64, f64) {
    let work_w = usable_work(work_width, MIN_WINDOW_W);
    let work_h = usable_work(work_height, MIN_WINDOW_H);
    if image_width == 0 || image_height == 0 {
        return (MIN_WINDOW_W.min(work_w), MIN_WINDOW_H.min(work_h));
    }
    let image_w = f64::from(image_width);
    let image_h = f64::from(image_height);
    let scale = (work_w * WORK_AREA_FRACTION / image_w)
        .min(work_h * WORK_AREA_FRACTION / image_h)
        .min(MAX_WINDOW_W / image_w)
        .min(MAX_WINDOW_H / image_h);
    let width = (image_w * scale)
        .round()
        .clamp(MIN_WINDOW_W, MAX_WINDOW_W)
        .min(work_w);
    let height = (image_h * scale)
        .round()
        .clamp(MIN_WINDOW_H, MAX_WINDOW_H)
        .min(work_h);
    (width.max(1.0), height.max(1.0))
}

/// A finite, positive work-area extent, or `fallback` (corrupt monitor data never panics).
fn usable_work(value: f64, fallback: f64) -> f64 {
    if value.is_finite() && value >= 1.0 {
        value
    } else {
        fallback
    }
}

/// Whether `path` is a file strictly inside `dir` (both canonicalized, case-folded for Windows).
///
/// Canonicalization fails for missing files and for inaccessible paths, which is exactly the
/// conservative "not owned" answer. The comparison is deliberately case-insensitive: Windows paths
/// are case-insensitive and a watched file can reach us with a different component case.
///
/// Shared with [`crate::trim`], which applies the same prefix rule to the recordings folder before
/// it replaces a video file.
pub(crate) fn path_is_inside(path: &Path, dir: &Path) -> bool {
    let (Ok(path), Ok(dir)) = (path.canonicalize(), dir.canonicalize()) else {
        return false;
    };
    let path = path.to_string_lossy().to_lowercase().replace('/', "\\");
    let dir = dir.to_string_lossy().to_lowercase().replace('/', "\\");
    path.len() > dir.len() && path.starts_with(&dir) && path.as_bytes()[dir.len()] == b'\\'
}

/// Whether the item's file belongs to Hangline (markup/trim may rewrite it in place).
fn item_owned(path: &Path) -> bool {
    match capture::screenshots_dir() {
        Ok(dir) => path_is_inside(path, &dir),
        Err(error) => {
            log::warn!("annotate: cannot resolve the screenshot directory: {error:#}");
            false
        }
    }
}

/// Mirrors the editor's dirty state from the frontend (`set_editor_dirty` command).
pub fn set_dirty(dirty: bool) {
    EDITOR_DIRTY.store(dirty, Ordering::Relaxed);
}

/// Editor ready handshake: the page registered its listener, so replay the last target.
///
/// Returns the last requested item id, if any. The page compares it with the id from its URL and
/// loads it only when they differ, which makes a target emitted while the page was still loading
/// (before `annotate-target` could be delivered) recoverable without a second event.
pub fn report_ready(app: &AppHandle) -> Option<String> {
    EDITOR_READY.store(true, Ordering::Relaxed);
    log::debug!("annotate: editor page ready");
    // Refresh the panel's mirror too: a panel that missed the creation event still learns the
    // editor is open from this handshake (the window must exist for the page to call it).
    emit_editor_state(app, true);
    EDITOR_TARGET.lock().ok().and_then(|target| target.clone())
}

/// Emits the panel's `editor-state` mirror; a failure is logged but never fails the command.
fn emit_editor_state(app: &AppHandle, open: bool) {
    if let Err(error) = app.emit(STATE_EVENT, EditorStateEvent { open }) {
        log::warn!("annotate: cannot emit {STATE_EVENT}: {error}");
    }
}

/// Clears the per-window editor bookkeeping when the window is destroyed.
fn reset_editor_state() {
    EDITOR_DIRTY.store(false, Ordering::Relaxed);
    EDITOR_READY.store(false, Ordering::Relaxed);
    if let Ok(mut target) = EDITOR_TARGET.lock() {
        *target = None;
    }
}

/// Resolves the editor target for `id` (used by the `get_editor_target` command).
pub fn target(app: &AppHandle, id: &str) -> anyhow::Result<EditorTarget> {
    let state = app
        .try_state::<AppState>()
        .ok_or_else(|| anyhow::anyhow!("annotate: the app state is not managed"))?;
    let item = state
        .item(id)
        .ok_or_else(|| anyhow::anyhow!("annotate: item {id} is not on the line"))?;
    let theme = state.snapshot().settings.theme;
    Ok(EditorTarget {
        owned: item_owned(&item.path),
        item,
        theme: theme.as_str().to_string(),
    })
}

/// Opens (or re-targets) the single markup editor window for item `id`.
///
/// Must run off the event-loop thread (the `open_annotate` command is `async`, which gives it a
/// worker thread; window creation dispatches to the loop and waits, see `capture::request_capture`).
pub fn open(app: &AppHandle, id: &str) -> anyhow::Result<()> {
    let target = target(app, id)?;
    let (theme, lang) = app
        .try_state::<AppState>()
        .map(|state| {
            let snapshot = state.snapshot();
            (snapshot.settings.theme, snapshot.resolved_language)
        })
        .unwrap_or((Theme::System, "en".to_string()));
    let monitor = editor_window::monitor(app);
    let (work_w, work_h) = editor_window::work_area_css(monitor.as_ref());
    let (width, height) = editor_window_size(
        target.item.width,
        target.item.height,
        work_w,
        work_h,
    );

    if let Some(window) = app.get_webview_window(LABEL) {
        // Never silently discard unsaved edits: focus the editor and ignore the new target until
        // the page reports clean again. The window keeps the geometry of the item it still shows.
        if EDITOR_DIRTY.load(Ordering::Relaxed) {
            log::info!(
                "annotate: ignoring the request to switch to {} while the editor has unsaved edits",
                target.item.path.display()
            );
            window.unminimize().ok();
            window.show().ok();
            window.set_focus().ok();
            emit_editor_state(app, true);
            return Ok(());
        }
        log::info!(
            "annotate: editor already open; switching it to {} ({width}x{height} css px)",
            target.item.path.display()
        );
        if let Ok(mut last) = EDITOR_TARGET.lock() {
            *last = Some(target.item.id.clone());
        }
        editor_window::apply_geometry(&window, monitor.as_ref(), width, height, "annotate")?;
        // Before the page's ready handshake an event cannot be delivered yet; `report_ready`
        // replays the stored target instead.
        if EDITOR_READY.load(Ordering::Relaxed) {
            if let Err(error) = window.emit(TARGET_EVENT, &target.item.id) {
                log::warn!("annotate: cannot emit {TARGET_EVENT}: {error}");
            }
        }
        window.unminimize().ok();
        window.show().ok();
        window.set_focus().ok();
        emit_editor_state(app, true);
        return Ok(());
    }

    if let Ok(mut last) = EDITOR_TARGET.lock() {
        *last = Some(target.item.id.clone());
    }
    EDITOR_READY.store(false, Ordering::Relaxed);
    EDITOR_DIRTY.store(false, Ordering::Relaxed);
    let url = annotate_url(&target.item.id, theme, &lang);
    let mut builder = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App(url.into()))
        .title(WINDOW_TITLE)
        .inner_size(width, height)
        .decorations(false)
        .transparent(false)
        .resizable(true)
        .always_on_top(false)
        .focusable(true)
        .visible(true);
    // Debug-only runtime-evidence hook; no-op in release builds (see `devtools`).
    if let Some(args) = crate::devtools::cdp_browser_args() {
        builder = builder.additional_browser_args(&args);
    }
    let window = builder
        .build()
        .context("annotate: cannot create the editor window")?;
    // The window owns no global state: closing it (Esc, titlebar X, Alt+F4) just destroys it and the
    // original stays on disk until the frontend's explicit Save. The Destroyed event also tells the
    // panel to resume card motion and lets the next `open_annotate` create a fresh window.
    let handle = app.clone();
    window.on_window_event(move |event| {
        match event {
            tauri::WindowEvent::CloseRequested { .. } => {
                log::info!("annotate: editor window close requested ({event:?})");
            }
            tauri::WindowEvent::Destroyed => {
                log::info!("annotate: editor window destroyed");
                reset_editor_state();
                emit_editor_state(&handle, false);
            }
            _ => {}
        }
    });
    if let Err(error) = editor_window::apply_geometry(&window, monitor.as_ref(), width, height, "annotate") {
        // A half-created window must not stay visible without its editor state bookkeeping.
        let _ = window.destroy();
        return Err(error);
    }
    let _ = window.set_focus();
    emit_editor_state(app, true);
    log::info!(
        "annotate: editor opened for {} ({width}x{height} css px, {}x{} image, owned={})",
        target.item.path.display(),
        target.item.width,
        target.item.height,
        target.owned
    );
    Ok(())
}

/// Whether the markup editor window currently exists (used by `get_editor_state`).
pub fn is_open(app: &AppHandle) -> bool {
    app.get_webview_window(LABEL).is_some()
}

/// Closes the editor window when one is open (Esc/Cancel and the successful-save path).
pub fn close(app: &AppHandle) {
    let Some(window) = app.get_webview_window(LABEL) else {
        log::debug!("annotate: close_editor with no editor window open");
        return;
    };
    match window.close() {
        Ok(()) => log::info!("annotate: editor window closed by the frontend"),
        Err(error) => log::warn!("annotate: cannot close the editor window: {error}"),
    }
}

/// The pixels and metadata produced by one successful write-back.
struct Written {
    /// The decoded, re-encoded image (width/height after the frontend's crop).
    image: RgbaImage,
    /// BLAKE3-64 of the exact PNG bytes now on disk.
    hash: u64,
    /// Refreshed 432 px preview, when the cache accepted it.
    thumb: Option<PathBuf>,
}

/// Saves one editor PNG over item `id`'s file and updates the item + broadcast snapshot.
///
/// This is the command body; it resolves the two data roots and then runs the testable
/// [`save_with_roots`] pipeline. Only the final state broadcast is Tauri-specific.
pub fn save(app: &AppHandle, id: &str, png_base64: &str) -> anyhow::Result<Item> {
    let state = app
        .try_state::<AppState>()
        .ok_or_else(|| anyhow::anyhow!("annotate: the app state is not managed"))?;
    let screenshots = capture::screenshots_dir()
        .context("annotate: the screenshot folder is unavailable")?;
    let cache = thumbnails::cache_dir().context("annotate: the thumbnail cache is unavailable")?;
    let item = save_with_roots(&state, id, png_base64, &screenshots, &cache)?;
    log::info!(
        "annotate: wrote {}x{} px into {} (hash {:#018x}, updatedAt {})",
        item.width,
        item.height,
        item.path.display(),
        item.hash,
        item.updated_at
    );

    match state.replace_item(item.clone()) {
        Ok(_) => {
            if app_state::emit_state(app).is_none() {
                log::warn!("annotate: saved but the state event could not be emitted");
            }
            Ok(state.item(id).unwrap_or(item))
        }
        Err(error) => {
            // `replace_item` mutates memory before persisting; publish whatever is current so the
            // panel cannot keep a card that disagrees with the file that was just written.
            let _ = app_state::emit_state(app);
            Err(error)
        }
    }
}

/// Command-level save pipeline with explicit data roots: ownership guard → write-back → metadata.
///
/// The ownership check lives here (not only in `get_editor_target`) and must stay in front of
/// [`write_back`]: a watched-folder file may never be rewritten, even when the frontend would send
/// the command directly. The unit test calls this function with a temporary screenshot root, so
/// removing the guard fails the test instead of only failing a `path_is_inside` helper test.
fn save_with_roots(
    state: &AppState,
    id: &str,
    png_base64: &str,
    screenshots_root: &Path,
    cache: &Path,
) -> anyhow::Result<Item> {
    let mut item = state
        .item(id)
        .ok_or_else(|| anyhow::anyhow!("annotate: item {id} is not on the line"))?;
    if !path_is_inside(&item.path, screenshots_root) {
        bail!(
            "annotate: refusing to rewrite {} (outside Hangline's screenshot folder)",
            item.path.display()
        );
    }
    let written = write_back(&item.path, png_base64, cache, &item.id)?;
    item.width = written.image.width();
    item.height = written.image.height();
    item.hash = written.hash;
    // Monotonic: two saves inside the same millisecond must still produce a new `?v=` URL.
    item.updated_at = item::now_ms().max(item.updated_at.saturating_add(1));
    // A failed forced refresh drops the cached preview instead of keeping pre-edit bytes behind the
    // new URL; the card then falls back to the (already replaced) original.
    item.thumb_path = written.thumb;
    Ok(item)
}

/// Decodes `png_base64`, atomically replaces `target`, hashes the new bytes and rebuilds the
/// 432 px preview. On any error the original file is untouched and the temp file is removed.
fn write_back(
    target: &Path,
    png_base64: &str,
    cache: &Path,
    id: &str,
) -> anyhow::Result<Written> {
    let payload = strip_data_url(png_base64.trim());
    if payload.len() > MAX_PAYLOAD_BYTES {
        bail!(
            "annotate: payload is {} bytes, the maximum is {MAX_PAYLOAD_BYTES}",
            payload.len()
        );
    }
    let bytes = BASE64_STANDARD
        .decode(payload)
        .context("annotate: the payload is not valid base64")?;
    if bytes.len() as u64 > MAX_DECODED_BYTES {
        bail!(
            "annotate: decoded payload is {} bytes, the maximum is {MAX_DECODED_BYTES}",
            bytes.len()
        );
    }
    let image = decode_limited(&bytes)?;
    let png = encode_png(&image)?;

    let tmp = temp_path(target);
    let replaced = write_synced(&tmp, &png).and_then(|()| {
        fs::rename(&tmp, target).with_context(|| {
            format!(
                "annotate: cannot replace {} with {}",
                target.display(),
                tmp.display()
            )
        })
    });
    if let Err(error) = replaced {
        // The original is still intact; a stale temp file must never masquerade as a screenshot.
        if let Err(remove_error) = fs::remove_file(&tmp) {
            if remove_error.kind() != std::io::ErrorKind::NotFound {
                log::warn!(
                    "annotate: cannot remove temp file {}: {remove_error}",
                    tmp.display()
                );
            }
        }
        return Err(error);
    }

    let hash = dedupe::hash_bytes(&png);
    let thumb = thumbnails::refresh_from_image(cache, target, id, &image);
    if thumb.is_none() {
        log::warn!("annotate: the 432 px preview could not be refreshed for {id}");
    }
    Ok(Written {
        image,
        hash,
        thumb,
    })
}

/// Accepts an optional `data:image/png;base64,` prefix (the frontend normally strips it).
fn strip_data_url(payload: &str) -> &str {
    if let Some(rest) = payload.strip_prefix("data:") {
        if let Some((_, data)) = rest.split_once(',') {
            return data;
        }
    }
    payload
}

/// Decodes the payload with the P3 caps enforced **before** the pixel buffer is allocated.
///
/// `image::load_from_memory` used to decode first and check the resulting dimensions afterwards:
/// a 12000×12000 PNG would allocate ~576 MB before `check_limits` rejected it. The reader is
/// therefore limited twice:
///
/// 1. `into_dimensions` parses only the header (with `max_image_width`/`max_image_height`, which are
///    strict for every decoder) and `check_limits` rejects an illegal edge or a decoded RGBA size
///    over 128 MB before any pixel buffer exists;
/// 2. the decoding reader gets `max_alloc = MAX_DECODED_BYTES`, so even a format whose header lies
///    about the pixel data cannot allocate more than the accepted budget.
fn decode_limited(bytes: &[u8]) -> anyhow::Result<RgbaImage> {
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(MAX_DECODED_BYTES);
    limits.max_image_width = Some(MAX_IMAGE_EDGE);
    limits.max_image_height = Some(MAX_IMAGE_EDGE);

    // Header pass: only the dimensions are read; the payload itself is not decoded yet.
    let mut probe = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .context("annotate: the payload is not a decodable image")?;
    probe.limits(limits.clone());
    let (width, height) = probe
        .into_dimensions()
        .context("annotate: the payload is not a decodable image")?;
    check_limits(width, height)?;

    // Decode pass with the same caps: `max_alloc` bounds the decoded buffer allocation.
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .context("annotate: the payload is not a decodable image")?;
    reader.limits(limits);
    let decoded = reader
        .decode()
        .context("annotate: the payload is not a decodable image")?;
    Ok(decoded.to_rgba8())
}

/// Enforces the image-size limits (plan P3-3: 12000×12000 and 128 MB decoded RGBA).
fn check_limits(width: u32, height: u32) -> anyhow::Result<()> {
    if width == 0 || height == 0 {
        bail!("annotate: the image has an empty dimension ({width}x{height})");
    }
    if width > MAX_IMAGE_EDGE || height > MAX_IMAGE_EDGE {
        bail!(
            "annotate: the image is {width}x{height}, the maximum edge is {MAX_IMAGE_EDGE}"
        );
    }
    let rgba_bytes = u64::from(width) * u64::from(height) * 4;
    if rgba_bytes > MAX_DECODED_BYTES {
        bail!(
            "annotate: the decoded image needs {rgba_bytes} bytes, the maximum is {MAX_DECODED_BYTES}"
        );
    }
    Ok(())
}

/// Removes stale write-back temp files from `dir` and returns the number removed.
///
/// Mirrors the thumbnail sweep: only names this module creates (`.hangline-markup-*.png.tmp`) and
/// only files older than [`STALE_TEMP_AGE`], so a fresh temp of a concurrent process is left alone.
/// A crash mid-save leaves the original untouched but can strand its temp file; that residue must
/// not survive a restart in the user's screenshot folder.
fn sweep_stale_temp_files_in(dir: &Path) -> usize {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    let mut removed = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !name.starts_with(TEMP_PREFIX) || !name.ends_with(".png.tmp") {
            continue;
        }
        let stale = entry
            .metadata()
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age > STALE_TEMP_AGE);
        if stale && fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    if removed > 0 {
        log::info!("annotate: swept {removed} stale markup temp file(s) from {}", dir.display());
    }
    removed
}

/// Startup sweep of the screenshot folder (called from `lib::run` after the state is managed).
pub fn sweep_stale_temp_files() {
    match capture::screenshots_dir() {
        Ok(dir) => {
            sweep_stale_temp_files_in(&dir);
        }
        Err(error) => log::warn!("annotate: cannot sweep markup temp files: {error:#}"),
    }
}

/// Re-encodes the composited image as a canonical RGBA8 PNG (the bytes written to disk).
fn encode_png(image: &RgbaImage) -> anyhow::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    PngEncoder::new(&mut bytes)
        .write_image(
            image.as_raw(),
            image.width(),
            image.height(),
            ExtendedColorType::Rgba8,
        )
        .context("annotate: cannot encode the composited image as PNG")?;
    Ok(bytes)
}

/// Writes `bytes` to `path` and flushes them to disk before the rename.
fn write_synced(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let mut file = fs::File::create(path)
        .with_context(|| format!("annotate: cannot create {}", path.display()))?;
    file.write_all(bytes)
        .with_context(|| format!("annotate: cannot write {}", path.display()))?;
    file.flush()
        .with_context(|| format!("annotate: cannot flush {}", path.display()))?;
    file.sync_all()
        .with_context(|| format!("annotate: cannot sync {}", path.display()))?;
    Ok(())
}

/// Same-directory temp path for the atomic replace (unique per call, hidden dot-file name).
fn temp_path(target: &Path) -> PathBuf {
    let dir = target
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut hasher = blake3::Hasher::new();
    hasher.update(&item::now_ms().to_le_bytes());
    hasher.update(&std::process::id().to_le_bytes());
    hasher.update(&TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed).to_le_bytes());
    hasher.update(target.as_os_str().to_string_lossy().as_bytes());
    let token = hasher.finalize().to_hex();
    dir.join(format!("{TEMP_PREFIX}{}.png.tmp", &token[..32]))
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;
    use std::time::{Duration, SystemTime};

    use crate::item::ItemKind;
    use crate::store;

    const TEST_ID: &str = "0123456789abcdef0123456789abcdef";

    /// Scratch directory under `target/` (git-ignored); never touches `%LOCALAPPDATA%`.
    fn test_dir(tag: &str) -> PathBuf {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("annotate-tests")
            .join(format!("{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create the annotate test dir");
        dir
    }

    /// Base64 of `image` encoded exactly as [`write_back`] would receive it from the page.
    fn page_payload(image: &RgbaImage) -> String {
        BASE64_STANDARD.encode(encode_png(image).expect("encode fixture"))
    }

    /// File names in `dir` that belong to the write-back (PNGs and temp files), sorted.
    fn write_back_files(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .expect("read the test dir")
            .flatten()
            .filter(|entry| entry.path().is_file())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".png") || name.ends_with(".tmp"))
            .collect();
        names.sort();
        names
    }

    #[test]
    fn window_url_carries_the_id_theme_and_language() {
        for (theme, wire) in [
            (Theme::System, "system"),
            (Theme::Light, "light"),
            (Theme::Dark, "dark"),
        ] {
            let url = annotate_url(TEST_ID, theme, "zh-Hans");
            assert!(url.starts_with("index.html?"), "url: {url}");
            assert!(url.contains("window=annotate"), "url: {url}");
            assert!(url.contains(&format!("id={TEST_ID}")), "url: {url}");
            assert!(url.contains(&format!("theme={wire}")), "url: {url}");
            assert!(
                url.contains("lang=zh-Hans"),
                "the first paint must know the resolved language without IPC: {url}"
            );
        }
    }

    #[test]
    fn window_size_fits_80_percent_caps_at_1400x900_and_keeps_aspect() {
        // 1920×1080 screenshot on a 1920×1040 work area: height is the tighter 80 % constraint and
        // the 1400 px cap decides; the window keeps the 16:9 ratio (1400×788).
        assert_eq!(
            editor_window_size(1920, 1080, 1920.0, 1040.0),
            (1400.0, 788.0)
        );
        // 4K on a 2560×1440 work area: the 1400×900 cap wins before the 80 % fit.
        assert_eq!(
            editor_window_size(3840, 2160, 2560.0, 1440.0),
            (1400.0, 788.0)
        );
        // Portrait 1500×2000 on a 1920×1040 work area: 80 % height → 832 px, aspect kept.
        assert_eq!(
            editor_window_size(1500, 2000, 1920.0, 1040.0),
            (624.0, 832.0)
        );
        // A small screenshot is upscaled to the 80 % fit (the editor should use the room).
        assert_eq!(
            editor_window_size(100, 50, 1920.0, 1040.0),
            (1400.0, 700.0)
        );
        // Extreme ratio: the 480 px minimum clamp wins on one axis, breaking strict aspect (the
        // image itself is still contained by the page's `object-fit: contain`).
        assert_eq!(
            editor_window_size(100, 20000, 1920.0, 1040.0),
            (480.0, 832.0)
        );
        // Unknown dimensions (0×0) and a work area smaller than the minimum still yield a usable
        // window; the work area always wins over the minimum.
        assert_eq!(editor_window_size(0, 0, 1920.0, 1040.0), (480.0, 360.0));
        assert_eq!(editor_window_size(800, 600, 300.0, 200.0), (300.0, 200.0));
        assert_eq!(
            editor_window_size(800, 600, f64::NAN, f64::INFINITY),
            (480.0, 360.0)
        );
    }

    #[test]
    fn ownership_is_a_canonical_prefix_check() {
        let dir = test_dir("ownership");
        let screenshots = dir.join("Screenshots");
        fs::create_dir_all(screenshots.join("nested")).expect("create the screenshot dir");
        let inside = screenshots.join("nested").join("shot.png");
        fs::write(&inside, b"png").expect("write the inside file");
        let outside = dir.join("elsewhere.png");
        fs::write(&outside, b"png").expect("write the outside file");

        assert!(path_is_inside(&inside, &screenshots));
        assert!(
            !path_is_inside(&outside, &screenshots),
            "a sibling directory is not owned"
        );
        assert!(
            !path_is_inside(&dir.join("missing.png"), &screenshots),
            "a missing file is never owned"
        );
        assert!(
            !path_is_inside(&screenshots, &screenshots),
            "the directory itself is not an owned file"
        );
        // Windows paths are case-insensitive: an upper-cased component still resolves inside.
        let upper = screenshots
            .join("NESTED")
            .join("SHOT.PNG");
        assert!(
            path_is_inside(&upper, &screenshots),
            "a case-variant path must still be recognized as owned"
        );
    }

    #[test]
    fn limits_guard_the_edge_and_the_decoded_size() {
        assert!(check_limits(4000, 3000).is_ok());
        assert!(check_limits(12000, 100).is_ok(), "12000 px on one edge is allowed");
        assert!(check_limits(0, 100).is_err(), "an empty dimension is rejected");
        assert!(check_limits(12001, 1).is_err(), "one pixel over the edge limit");
        assert!(
            check_limits(6000, 6000).is_err(),
            "36 Mpx need 144 MB decoded RGBA, over the 128 MB cap"
        );
    }

    #[test]
    fn write_back_replaces_the_original_atomically_and_refreshes_the_preview() {
        let dir = test_dir("write-back");
        let cache = dir.join("cache");
        let target = dir.join("shot.png");
        RgbaImage::from_pixel(10, 10, image::Rgba([0, 0, 255, 255]))
            .save(&target)
            .expect("seed the original");

        let rewritten = RgbaImage::from_pixel(20, 30, image::Rgba([255, 0, 0, 255]));
        let written = write_back(&target, &page_payload(&rewritten), &cache, TEST_ID)
            .expect("write back succeeds");

        assert_eq!((written.image.width(), written.image.height()), (20, 30));
        let on_disk = image::open(&target).expect("the replaced file is a readable PNG").to_rgba8();
        assert_eq!((on_disk.width(), on_disk.height()), (20, 30));
        assert_eq!(on_disk.get_pixel(0, 0).0, [255, 0, 0, 255]);
        assert_eq!(
            written.hash,
            dedupe::hash_bytes(&fs::read(&target).expect("read the replaced file")),
            "the hash must describe the exact bytes now on disk"
        );
        let thumb = written.thumb.expect("the 432 px preview is refreshed");
        assert_eq!(
            image::open(&thumb).expect("open the refreshed preview").width(),
            20
        );
        assert_eq!(
            write_back_files(&dir),
            vec!["shot.png".to_string()],
            "the atomic replace must leave no .tmp residue"
        );
        assert!(
            !fs::read_dir(&cache)
                .expect("read the cache")
                .flatten()
                .any(|entry| entry.file_name().to_string_lossy().ends_with(".tmp")),
            "the preview write must leave no .tmp residue"
        );
    }

    #[test]
    fn invalid_payloads_leave_the_original_untouched() {
        let dir = test_dir("invalid-payloads");
        let cache = dir.join("cache");
        let target = dir.join("shot.png");
        let original = page_payload(&RgbaImage::from_pixel(8, 8, image::Rgba([1, 2, 3, 255])));
        fs::write(&target, BASE64_STANDARD.decode(&original).expect("seed bytes"))
            .expect("seed the original");
        let before = fs::read(&target).expect("read the original");

        let too_wide = page_payload(&RgbaImage::from_pixel(12001, 1, image::Rgba([9, 9, 9, 255])));
        let garbage = BASE64_STANDARD.encode(b"this is not a png");
        for (case, payload) in [
            ("not base64", "%%% definitely not base64 %%%".to_string()),
            ("not an image", garbage),
            ("edge over the limit", too_wide),
            ("empty", String::new()),
        ] {
            let error = write_back(&target, &payload, &cache, TEST_ID)
                .err()
                .unwrap_or_else(|| panic!("{case} must be rejected"));
            assert!(!format!("{error:#}").is_empty());
        }
        // None of the calls above may touch the original file, the temp files or the preview.
        assert_eq!(fs::read(&target).expect("re-read the original"), before);
        assert_eq!(write_back_files(&dir), vec!["shot.png".to_string()]);
        assert!(!cache.exists(), "a rejected payload must not write a preview");
    }

    #[test]
    fn data_url_prefixes_are_tolerated() {
        let dir = test_dir("data-url");
        let cache = dir.join("cache");
        let target = dir.join("shot.png");
        RgbaImage::from_pixel(4, 4, image::Rgba([0, 0, 0, 255]))
            .save(&target)
            .expect("seed the original");
        let image = RgbaImage::from_pixel(5, 5, image::Rgba([255, 255, 255, 255]));
        let payload = format!("data:image/png;base64,{}", page_payload(&image));
        let written = write_back(&target, &payload, &cache, TEST_ID).expect("data URL payload");
        assert_eq!((written.image.width(), written.image.height()), (5, 5));
    }

    /// P3 DoD: 100 consecutive saves must all succeed and leave exactly one file (no `.tmp`).
    #[test]
    fn one_hundred_write_backs_all_succeed_without_residue() {
        let dir = test_dir("stress");
        let cache = dir.join("cache");
        let target = dir.join("shot.png");
        RgbaImage::from_pixel(640, 480, image::Rgba([0, 0, 0, 255]))
            .save(&target)
            .expect("seed the original");

        let mut failures = 0usize;
        for index in 0..100u32 {
            let shade = (index % 256) as u8;
            let image =
                RgbaImage::from_pixel(640, 480, image::Rgba([shade, 128, 255 - shade, 255]));
            if let Err(error) = write_back(&target, &page_payload(&image), &cache, TEST_ID) {
                failures += 1;
                log::error!("annotate stress: save {index} failed: {error:#}");
            }
        }
        assert_eq!(failures, 0, "the P3 definition of done is 0/100 failures");
        assert_eq!(
            write_back_files(&dir),
            vec!["shot.png".to_string()],
            "exactly one file and no temp residue after 100 atomic replacements"
        );
        let on_disk = image::open(&target).expect("the final file is a readable PNG").to_rgba8();
        assert_eq!((on_disk.width(), on_disk.height()), (640, 480));
        assert!(cache.join(format!("{TEST_ID}.png")).exists());
        assert!(
            !cache.join(format!("{TEST_ID}.png.tmp")).exists(),
            "the forced preview refresh must not leave a temp file"
        );
    }

    /// Builds a minimal, well-formed `item` for command-level state tests.
    fn item_at(id: &str, path: &Path, kind: ItemKind) -> Item {
        Item {
            id: id.to_string(),
            path: path.to_path_buf(),
            thumb_path: None,
            kind,
            hash: 7,
            width: 10,
            height: 10,
            created_at: 1,
            added_at: 2,
            updated_at: 0,
        }
    }

    /// One PNG chunk with its length and CRC (for synthetic oversized headers).
    fn png_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(12 + data.len());
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        let mut crc = 0xFFFF_FFFFu32;
        for byte in kind.iter().chain(data.iter()) {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        out.extend_from_slice(&(crc ^ 0xFFFF_FFFF).to_be_bytes());
        out
    }

    /// A header-only RGBA PNG claiming `width`×`height` (no pixel data; never decoded).
    fn png_header_payload(width: u32, height: u32) -> String {
        let mut ihdr = Vec::with_capacity(13);
        ihdr.extend_from_slice(&width.to_be_bytes());
        ihdr.extend_from_slice(&height.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit RGBA, deflate, adaptive filter, no interlace
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend_from_slice(&png_chunk(b"IHDR", &ihdr));
        png.extend_from_slice(&png_chunk(b"IEND", &[]));
        BASE64_STANDARD.encode(png)
    }

    /// The command-level guard must reject a watched-folder item even though the file is readable
    /// and the payload valid: removing the `path_is_inside` check in `save_with_roots` fails this
    /// test (the watched file would be overwritten). The same pipeline still accepts an owned file.
    #[test]
    fn command_save_rejects_watched_folder_files_before_writing() {
        const WATCHED_ID: &str = "fedcba9876543210fedcba9876543210";
        const OWNED_ID: &str = "00112233445566778899aabbccddeeff";
        let root = test_dir("command-ownership");
        let screenshots = root.join("Screenshots");
        let watched = root.join("Watched");
        fs::create_dir_all(&screenshots).expect("create the screenshots root");
        fs::create_dir_all(&watched).expect("create the watched root");
        let owned_path = screenshots.join("owned.png");
        let watched_path = watched.join("watched.png");
        RgbaImage::from_pixel(10, 10, image::Rgba([1, 2, 3, 255]))
            .save(&owned_path)
            .expect("seed the owned file");
        RgbaImage::from_pixel(8, 8, image::Rgba([4, 5, 6, 255]))
            .save(&watched_path)
            .expect("seed the watched file");
        let watched_before = fs::read(&watched_path).expect("read the watched file");

        let state_dir = root.join("state");
        store::save_atomic(
            &state_dir,
            &[item_at(WATCHED_ID, &watched_path, ItemKind::WatchedFolder)],
        )
        .expect("seed the watched state");
        let state = AppState::load(state_dir.clone());
        let cache = root.join("cache");
        let payload = page_payload(&RgbaImage::from_pixel(20, 30, image::Rgba([9, 8, 7, 255])));

        let error = save_with_roots(&state, WATCHED_ID, &payload, &screenshots, &cache)
            .expect_err("a watched-folder item must never be rewritten");
        assert!(
            format!("{error:#}").contains("outside Hangline"),
            "unexpected error: {error:#}"
        );
        assert_eq!(
            fs::read(&watched_path).expect("re-read the watched file"),
            watched_before,
            "the rejected save must leave the watched file byte-identical"
        );
        assert_eq!(
            write_back_files(&watched),
            vec!["watched.png".to_string()],
            "a rejected save must not leave a temp file"
        );
        assert!(!cache.exists(), "a rejected save must not write a preview");

        // Control: the same pipeline accepts a file inside the screenshot root.
        store::save_atomic(
            &state_dir,
            &[item_at(OWNED_ID, &owned_path, ItemKind::Capture)],
        )
        .expect("seed the owned state");
        let state = AppState::load(state_dir.clone());
        let saved = save_with_roots(&state, OWNED_ID, &payload, &screenshots, &cache)
            .expect("an owned file is writable");
        assert_eq!((saved.width, saved.height), (20, 30));
        let on_disk = image::open(&owned_path)
            .expect("the owned file is a readable PNG")
            .to_rgba8();
        assert_eq!((on_disk.width(), on_disk.height()), (20, 30));
        assert!(
            cache.join(format!("{OWNED_ID}.png")).exists(),
            "the owned save refreshes the preview"
        );
    }

    /// Oversized headers are rejected before any pixel buffer is allocated: one over the strict
    /// 12000 px edge and one whose decoded RGBA would need more than the 128 MB cap.
    #[test]
    fn oversized_dimension_headers_are_rejected_before_decoding() {
        let dir = test_dir("header-limits");
        let cache = dir.join("cache");
        let target = dir.join("shot.png");
        RgbaImage::from_pixel(10, 10, image::Rgba([0, 0, 0, 255]))
            .save(&target)
            .expect("seed the original");
        let before = fs::read(&target).expect("read the original");

        for (case, width, height) in [
            ("edge over the limit", 12_001, 1),
            ("decoded size over the limit", 6_000, 6_000),
        ] {
            let payload = png_header_payload(width, height);
            let error = write_back(&target, &payload, &cache, TEST_ID)
                .err()
                .unwrap_or_else(|| panic!("{case} must be rejected from the header"));
            assert!(
                !format!("{error:#}").is_empty(),
                "{case} must carry a diagnostic"
            );
        }
        assert_eq!(
            fs::read(&target).expect("re-read the original"),
            before,
            "header-only rejections must not touch the original"
        );
        assert_eq!(write_back_files(&dir), vec!["shot.png".to_string()]);
        assert!(!cache.exists(), "header-only rejections must not write a preview");
    }

    /// The startup sweep removes only old `.hangline-markup-*.png.tmp` residue: a fresh temp may
    /// belong to a concurrent save and unrelated files are never touched.
    #[test]
    fn startup_sweep_removes_only_stale_markup_temps() {
        let dir = test_dir("temp-sweep");
        let stale = dir.join(".hangline-markup-deadbeef.png.tmp");
        let fresh = dir.join(".hangline-markup-feedface.png.tmp");
        let screenshot = dir.join("keep.png");
        fs::write(&stale, b"stale").expect("write the stale temp");
        fs::write(&fresh, b"fresh").expect("write the fresh temp");
        fs::write(&screenshot, b"png").expect("write the screenshot");
        let old = SystemTime::now() - Duration::from_secs(2 * 60 * 60);
        fs::OpenOptions::new()
            .write(true)
            .open(&stale)
            .expect("open the stale temp")
            .set_modified(old)
            .expect("age the stale temp");

        assert_eq!(sweep_stale_temp_files_in(&dir), 1);
        assert!(!stale.exists(), "old markup residue must be swept");
        assert!(fresh.exists(), "a fresh temp may still be an in-flight write");
        assert!(screenshot.exists(), "the sweep must not touch real screenshots");
    }

    #[test]
    fn editor_state_payload_serializes_the_open_flag() {
        assert_eq!(
            serde_json::to_value(EditorStateEvent { open: true }).expect("serialize"),
            serde_json::json!({ "open": true })
        );
    }
}
