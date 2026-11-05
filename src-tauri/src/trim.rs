//! P3-6/P3-7 video trim: the borderless trim window, its target payload, and the WinRT export.
//!
//! # Window
//!
//! One single, borderless, focusable `trim` window (`index.html?window=trim&id=…`) sized from the
//! recording's aspect ratio (fitted into 900×600, at least 640×400) and centred on the monitor that
//! holds the panel. `open_trim` re-targets an existing window instead of opening a second one; a
//! `trim-target` event (or the ready-handshake replay) switches the page to a new item, exactly like
//! the markup editor.
//!
//! # Export pipeline
//!
//! Implemented on a `spawn_blocking` worker (never the Tauri event loop or the async reactor):
//!
//! 1. re-validate the item on the backend (it must still be a `Recording` inside
//!    `%USERPROFILE%\Videos\Captures`; the frontend's disabled Save button is not a boundary);
//! 2. enforce the P3 caps: file ≤ 2 GiB, duration ≤ 60 min, `0 ≤ start < end ≤ duration`, and
//!    `end > start + 100 ms`;
//! 3. `MediaComposition` + `MediaClip::CreateFromFileAsync`, append the clip, set
//!    `TrimTimeFromStart = start` / `TrimTimeFromEnd = duration − end`, and render through
//!    `RenderToFileAsync` into `‹dir›/.hangline-trim-‹token›‹ext›` (same directory, therefore the
//!    same volume, so the final rename is atomic);
//! 4. hash the rendered file (streamed BLAKE3, never buffered into memory), rename it over the
//!    original, regenerate the 432 px first-frame preview and repair the item metadata
//!    (`hash`, `width`/`height`, `thumbPath`, monotonic `updatedAt`), persist via
//!    `AppState::replace_item` and broadcast the new snapshot.
//!
//! Every failure removes the temp file and leaves the original byte-identical. Because the render
//! target is a dot-file, the folder watcher ignores it (`is_candidate` rejects dot-files and
//! `.tmp`/`.part`), and the replaced original is skipped by `contains_path`, so a trim never
//! duplicates a card.
//!
//! Rendering is a minutes-long operation for the supported 2 GiB / 60 min cap, so the export is
//! serialized by [`EXPORTING`]: `open_trim` refuses to re-target the window while a render is in
//! flight, `close_trim` refuses to close it, and the window's `CloseRequested` handler (Alt+F4 or
//! the taskbar) prevents the close. Once the atomic rename has happened the export is committed:
//! a later metadata persistence failure is reported as a logged warning and the updated item is
//! returned instead of an error, because no retry could undo or repeat the replace.
//!
//! # Validation order
//!
//! [`trim`] resolves the file size and the pure range rules *before* it asks WinRT for the clip
//! duration ([`validate_preflight`]): a >2 GiB file or an unordered range is rejected without a
//! media probe. The duration cap and `end ≤ duration` need the probe and are checked afterwards by
//! [`validate_trim`] (which composes both halves).
//!
//! # Output profile
//!
//! `MediaComposition::RenderToFileAsync(StorageFile)` uses the media runtime's default
//! `MediaEncodingProfile`: it re-encodes with the source frame size (the fixture stays 640×360),
//! copies the audio stream when the source has one (the runtime synthesizes a silent track for a
//! video-only source) and does not need a caller-supplied bitrate. Output size therefore differs
//! from the source even when the selected range is the whole clip.
//!
//! # Progress semantics
//!
//! `trim-progress { id, progress }` is coarse by design: `0.05` when the export starts, `0.6`
//! after `RenderToFileAsync` produced the temp file, and `1.0` once the original was atomically
//! replaced. WinRT's render operation does expose an `IAsyncOperationWithProgress` progress
//! handler, but its reporting granularity depends on the installed codecs/Media Foundation pipeline
//! and is not reliable enough to drive a fine-grained bar; the three milestones are guaranteed to
//! be emitted and are strictly increasing.
//!
//! # Runtime tests
//!
//! The WinRT tests are `#[ignore]` because they need a real media runtime and a video file. Generate
//! the neutral fixture with `scripts/p3/fixture-video.ps1`, then (from the repository root):
//!
//! ```powershell
//! .\scripts\p3\fixture-video.ps1
//! $env:HANG_TEST_VIDEO = "$PWD\src-tauri\target\p3-fixtures\p3-recording-fixture.mp4"
//! cargo test --manifest-path src-tauri\Cargo.toml trim:: -- --ignored --nocapture
//! ```
//!
//! The end-to-end harness (`scripts/p3/trim-e2e.mjs`) asserts the same duration with `ffprobe`
//! and verifies playability, the atomic replace, the refreshed preview and the panel card.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{bail, Context};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};
use windows::core::HSTRING;
use windows::Foundation::TimeSpan;
use windows::Media::Editing::MediaComposition;
use windows::Storage::{CreationCollisionOption, StorageFile, StorageFolder};

use crate::annotate::path_is_inside;
use crate::app_state::{self, AppState};
use crate::dedupe;
use crate::editor_window;
use crate::item::{self, Item, ItemKind};
use crate::thumbnails;
use crate::video::{self, Apartment};

/// Label of the trim window (single instance).
pub const LABEL: &str = "trim";

/// Window title (custom titlebar text comes from the frontend i18n table).
const WINDOW_TITLE: &str = "Hangline Trim";

/// Event asking an already-open trim window to switch to another item.
pub const TARGET_EVENT: &str = "trim-target";

/// Event telling the panel whether the trim window is open (P3-4 busy model extended by P3-6).
pub const STATE_EVENT: &str = "trim-state";

/// Export progress event (0–1; coarse milestones, see the module docs).
pub const PROGRESS_EVENT: &str = "trim-progress";

/// Payload of [`STATE_EVENT`].
#[derive(Clone, Copy, Debug, Serialize)]
pub struct TrimStateEvent {
    /// True while the single trim window exists.
    pub open: bool,
}

/// Payload of [`PROGRESS_EVENT`].
#[derive(Clone, Debug, Serialize)]
pub struct TrimProgressEvent {
    /// The item being exported.
    pub id: String,
    /// Coarse milestone in `[0, 1]`: 0.05 start, 0.6 rendered, 1.0 replaced.
    pub progress: f64,
}

/// Maximum trim content width (plan P3-6 / design spec §3.8).
pub const MAX_WINDOW_W: f64 = 900.0;
/// Maximum trim content height.
pub const MAX_WINDOW_H: f64 = 600.0;
/// Minimum trim content width.
pub const MIN_WINDOW_W: f64 = 640.0;
/// Minimum trim content height.
pub const MIN_WINDOW_H: f64 = 400.0;

/// Maximum video file size accepted for trimming: 2 GiB (design spec §3.8 "2 GB").
pub const MAX_TRIM_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Maximum video duration accepted for trimming: 60 minutes.
pub const MAX_TRIM_DURATION_MS: u64 = 60 * 60 * 1000;
/// Minimum selection length. The range is *exclusive*: `end − start` must be strictly greater.
pub const MIN_TRIM_RANGE_MS: u64 = 100;

/// Prefix of the same-directory render temp file (hidden so folder watchers ignore it).
const TEMP_PREFIX: &str = ".hangline-trim-";

/// Sequence making temp names unique when two exports land in the same millisecond.
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Age after which a crashed export temp file is swept at startup (markup-sweep parity).
const STALE_TEMP_AGE: Duration = Duration::from_secs(60 * 60);

/// Whether the trim page registered its `trim-target` listener (ready handshake).
static TRIM_READY: AtomicBool = AtomicBool::new(false);

/// Last item id the page should show; replayed by the ready handshake.
static TRIM_TARGET: Mutex<Option<String>> = Mutex::new(None);

/// Whether a render/hash/replace is in flight. One at a time: a second export would fight over the
/// same original, and the window must not be re-targeted or closed while the bytes are being
/// rewritten (P3-6 review follow-up).
static EXPORTING: AtomicBool = AtomicBool::new(false);

/// What the trim page needs to render one recording (plan P3-6).
///
/// `sizeBytes`/`durationMs` are the values the backend validated; `maxBytes`/`maxDurationMs` let the
/// page refuse an unsupported file with the same limits (the backend rejects it again).
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrimTarget {
    /// The recording currently on the line.
    pub item: Item,
    /// Current file size in bytes.
    pub size_bytes: u64,
    /// Clip duration in milliseconds (`MediaClip.OriginalDuration`).
    pub duration_ms: u64,
    /// Backend limit the page mirrors (2 GiB).
    pub max_bytes: u64,
    /// Backend limit the page mirrors (60 min).
    pub max_duration_ms: u64,
    /// Raw `settings.theme` (`system`/`light`/`dark`); the page resolves `system` itself.
    pub theme: String,
}

/// A typed trim rejection (plan P3-7). The frontend shows one localized "Trim failed" message plus
/// the raw details in the log; the variants keep the reasons distinguishable in tests/logs.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum TrimError {
    /// The item is no longer on the line.
    #[error("trim: item {0} is not on the line")]
    NotFound(String),
    /// The item is not a recording (the image editor owns those).
    #[error("trim: item {0} is not a recording")]
    NotRecording(String),
    /// The recording file disappeared or is not a regular file.
    #[error("trim: the recording file is missing: {0}")]
    FileMissing(PathBuf),
    /// The file is outside the recordings folder, so Hangline must not replace it.
    #[error("trim: refusing to rewrite {} (outside the recordings folder)", .0.display())]
    NotOwned(PathBuf),
    /// The file exceeds [`MAX_TRIM_BYTES`].
    #[error("trim: the recording is {size_bytes} bytes, over the 2 GiB limit")]
    TooLarge {
        /// Actual file size in bytes.
        size_bytes: u64,
    },
    /// The duration exceeds [`MAX_TRIM_DURATION_MS`].
    #[error("trim: the recording is {duration_ms} ms long, over the 60 min limit")]
    TooLong {
        /// Actual clip duration in milliseconds.
        duration_ms: u64,
    },
    /// `start`/`end` is negative, unordered, or shorter than the 100 ms minimum.
    #[error("trim: invalid trim range {start_ms}–{end_ms} ms (start ≥ 0, end > start + 100)")]
    InvalidRange {
        /// Requested start in milliseconds (may be negative; rejected).
        start_ms: i64,
        /// Requested end in milliseconds.
        end_ms: i64,
    },
    /// `end` (or the whole range) lies past the clip duration.
    #[error("trim: range end {end_ms} ms is past the {duration_ms} ms clip")]
    RangeOutOfBounds {
        /// Requested end in milliseconds.
        end_ms: i64,
        /// Clip duration in milliseconds.
        duration_ms: u64,
    },
}

/// One validated selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrimRange {
    /// Absolute start offset in milliseconds.
    pub start_ms: u64,
    /// Absolute end offset in milliseconds.
    pub end_ms: u64,
}

impl TrimRange {
    /// Selected length in milliseconds.
    pub fn duration_ms(self) -> u64 {
        self.end_ms.saturating_sub(self.start_ms)
    }
}

/// One export plan: the original, the same-directory render temp, and the validated range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrimPlan {
    /// The file that will be replaced after a successful render.
    pub target: PathBuf,
    /// Hidden same-directory render destination (removed on every failure).
    pub temp: PathBuf,
    /// Validated selection.
    pub range: TrimRange,
}

/// The trim page URL (ids are opaque 32-hex strings; the theme/lang make the first paint correct).
pub(crate) fn trim_url(id: &str, theme: &str, lang: &str) -> String {
    format!("index.html?window=trim&id={id}&theme={theme}&lang={lang}")
}

/// Content size for one recording: its aspect ratio fitted into 900×600, at least 640×400.
///
/// The window keeps the video aspect for normal landscape/portrait recordings; an extreme ratio
/// hits the 640 px width (or 400 px height) minimum and the page still contains the video with
/// `object-fit: contain`. Unknown dimensions (0×0, a recording whose preview failed) open at the
/// minimum usable size instead of failing.
pub fn trim_window_size(width: u32, height: u32) -> (f64, f64) {
    if width == 0 || height == 0 {
        return (MIN_WINDOW_W, MIN_WINDOW_H);
    }
    let width = f64::from(width);
    let height = f64::from(height);
    let scale = (MAX_WINDOW_W / width).min(MAX_WINDOW_H / height);
    let css_width = (width * scale)
        .round()
        .clamp(MIN_WINDOW_W, MAX_WINDOW_W);
    let css_height = (height * scale)
        .round()
        .clamp(MIN_WINDOW_H, MAX_WINDOW_H);
    (css_width.max(1.0), css_height.max(1.0))
}

/// `%USERPROFILE%\Videos\Captures` (the only folder whose videos are `ItemKind::Recording`).
///
/// `None` when the environment is incomplete; the caller reports a clear error instead of guessing
/// a path. The folder is read from `USERPROFILE` on purpose: the isolated evidence runs relocate
/// the entire profile (never the real `Videos\Captures`).
pub fn recordings_dir() -> Option<PathBuf> {
    // Debug-only test hook: isolated evidence runs cannot override USERPROFILE (the log plugin's
    // known-folder log dir then fails startup), so the recordings watcher/folder is relocated
    // without touching the profile. Release builds always use the literal profile path.
    #[cfg(debug_assertions)]
    if let Some(dir) = std::env::var_os("HANG_RECORDINGS_DIR").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    std::env::var_os("USERPROFILE")
        .filter(|value| !value.is_empty())
        .map(|profile| PathBuf::from(profile).join("Videos").join("Captures"))
}

/// Rejects a file over [`MAX_TRIM_BYTES`] without touching the media stack (pure).
pub fn validate_size(size_bytes: u64) -> Result<(), TrimError> {
    if size_bytes > MAX_TRIM_BYTES {
        return Err(TrimError::TooLarge { size_bytes });
    }
    Ok(())
}

/// Rejects a clip over [`MAX_TRIM_DURATION_MS`] (pure, but needs the probed duration).
pub fn validate_duration(duration_ms: u64) -> Result<(), TrimError> {
    if duration_ms > MAX_TRIM_DURATION_MS {
        return Err(TrimError::TooLong { duration_ms });
    }
    Ok(())
}

/// Enforces `start ≥ 0`, `start < end` and `end > start + 100` and returns the pair as `u64`.
///
/// Equality on the 100 ms minimum is invalid (plan P3-7 / P3 reference). The duration-dependent
/// `end ≤ duration` check is separate so it can run only after the WinRT probe.
pub fn validate_range(start_ms: i64, end_ms: i64) -> Result<(u64, u64), TrimError> {
    if start_ms < 0 || end_ms < 0 {
        return Err(TrimError::InvalidRange { start_ms, end_ms });
    }
    let (start_ms, end_ms) = (start_ms as u64, end_ms as u64);
    if start_ms >= end_ms || end_ms - start_ms <= MIN_TRIM_RANGE_MS {
        return Err(TrimError::InvalidRange {
            start_ms: start_ms as i64,
            end_ms: end_ms as i64,
        });
    }
    Ok((start_ms, end_ms))
}

/// The checks that must run before the WinRT duration probe: size and range sanity.
///
/// A >2 GiB file or an impossible selection is refused without blocking on the media runtime; the
/// caller probes the duration only when this passes and then applies [`validate_trim`].
pub fn validate_preflight(size_bytes: u64, start_ms: i64, end_ms: i64) -> Result<(), TrimError> {
    validate_size(size_bytes)?;
    validate_range(start_ms, end_ms)?;
    Ok(())
}

/// Enforces the P3-7 caps and range rules (pure; the frontend mirrors the same limits).
///
/// The order is deliberate: file/duration limits first (they describe the file), then the range
/// (it describes the user's choice), then the duration-dependent bound. `start ≥ 0`, `start < end`,
/// `end > start + 100` and `end ≤ duration`; equality on the 100 ms minimum is invalid (plan P3-7 /
/// P3 reference).
pub fn validate_trim(
    size_bytes: u64,
    duration_ms: u64,
    start_ms: i64,
    end_ms: i64,
) -> Result<TrimRange, TrimError> {
    validate_size(size_bytes)?;
    validate_duration(duration_ms)?;
    let (start_ms, end_ms) = validate_range(start_ms, end_ms)?;
    if end_ms > duration_ms {
        return Err(TrimError::RangeOutOfBounds {
            end_ms: end_ms as i64,
            duration_ms,
        });
    }
    Ok(TrimRange { start_ms, end_ms })
}

/// Resolves the trim target for `id` (used by `get_trim_target` and `open_trim`).
///
/// Blocks on the WinRT media stack (`video::duration_ms`), so callers must run it on a blocking
/// worker thread — never on the Tauri event loop.
pub fn target(app: &AppHandle, id: &str) -> anyhow::Result<TrimTarget> {
    let state = app
        .try_state::<AppState>()
        .ok_or_else(|| anyhow::anyhow!("trim: the app state is not managed"))?;
    let item = state
        .item(id)
        .ok_or_else(|| TrimError::NotFound(id.to_string()))?;
    if item.kind != ItemKind::Recording {
        return Err(TrimError::NotRecording(id.to_string()).into());
    }
    let metadata = fs::metadata(&item.path).map_err(|_| TrimError::FileMissing(item.path.clone()))?;
    if !metadata.is_file() {
        return Err(TrimError::FileMissing(item.path.clone()).into());
    }
    let duration_ms = video::duration_ms(&item.path)?;
    let theme = state.snapshot().settings.theme;
    Ok(TrimTarget {
        item,
        size_bytes: metadata.len(),
        duration_ms,
        max_bytes: MAX_TRIM_BYTES,
        max_duration_ms: MAX_TRIM_DURATION_MS,
        theme: theme.as_str().to_string(),
    })
}

/// Opens (or re-targets) the single trim window for a resolved target.
///
/// Must run off the event-loop thread (window creation dispatches to the loop and waits). The
/// caller resolves [`TrimTarget`] on a blocking worker first because it queries the media stack.
pub fn open(app: &AppHandle, target: TrimTarget, lang: &str) -> anyhow::Result<()> {
    let (width, height) = trim_window_size(target.item.width, target.item.height);
    let monitor = editor_window::monitor(app);

    if let Some(window) = app.get_webview_window(LABEL) {
        // A re-target switches the page's item; doing that mid-export would let the user start a
        // second export while the first one owns the original and the progress bar. Refuse the
        // retarget (the window keeps showing and saving the item it already has).
        if exporting() {
            log::warn!(
                "trim: ignoring the request to switch to {} while an export is in flight",
                target.item.path.display()
            );
            window.unminimize().ok();
            window.show().ok();
            window.set_focus().ok();
            emit_trim_state(app, true);
            return Ok(());
        }
        log::info!(
            "trim: window already open; switching it to {} ({width}x{height} css px)",
            target.item.path.display()
        );
        if let Ok(mut last) = TRIM_TARGET.lock() {
            *last = Some(target.item.id.clone());
        }
        editor_window::apply_geometry(&window, monitor.as_ref(), width, height, "trim")?;
        // Before the page's ready handshake an event cannot be delivered yet; `report_ready`
        // replays the stored target instead.
        if TRIM_READY.load(Ordering::Relaxed) {
            if let Err(error) = window.emit(TARGET_EVENT, &target.item.id) {
                log::warn!("trim: cannot emit {TARGET_EVENT}: {error}");
            }
        }
        window.unminimize().ok();
        window.show().ok();
        window.set_focus().ok();
        emit_trim_state(app, true);
        return Ok(());
    }

    if let Ok(mut last) = TRIM_TARGET.lock() {
        *last = Some(target.item.id.clone());
    }
    TRIM_READY.store(false, Ordering::Relaxed);
    let url = trim_url(&target.item.id, &target.theme, lang);
    let mut builder = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App(url.into()))
        .title(WINDOW_TITLE)
        .inner_size(width, height)
        // The page's staged layout needs at least the 640×400 the size plan promises; without the
        // constraint a manual resize could clip the footer buttons.
        .min_inner_size(MIN_WINDOW_W, MIN_WINDOW_H)
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
    let window = builder.build().context("trim: cannot create the trim window")?;
    let handle = app.clone();
    window.on_window_event(move |event| {
        match event {
            tauri::WindowEvent::CloseRequested { api, .. } => {
                // Alt+F4 / taskbar close while the atomic replace is in flight would kill the
                // webview that reports the export and could leave the user without the result.
                if exporting() {
                    log::warn!("trim: refusing the window close request during an export");
                    api.prevent_close();
                } else {
                    log::info!("trim: window close requested ({event:?})");
                }
            }
            tauri::WindowEvent::Destroyed => {
                log::info!("trim: window destroyed");
                reset_trim_state();
                emit_trim_state(&handle, false);
            }
            _ => {}
        }
    });
    if let Err(error) = editor_window::apply_geometry(&window, monitor.as_ref(), width, height, "trim")
    {
        // A half-created window must not stay visible without its editor-state bookkeeping.
        let _ = window.destroy();
        return Err(error);
    }
    let _ = window.set_focus();
    emit_trim_state(app, true);
    log::info!(
        "trim: window opened for {} ({width}x{height} css px, {}x{} video, {:.1} s)",
        target.item.path.display(),
        target.item.width,
        target.item.height,
        target.duration_ms as f64 / 1000.0
    );
    Ok(())
}

/// Closes the trim window when one is open (Esc/Cancel and the successful-save path).
///
/// A close requested while an export is in flight is refused: the successful-save path calls this
/// only after `trim_video` resolved, so an in-flight close can only come from a stale page.
pub fn close(app: &AppHandle) {
    let Some(window) = app.get_webview_window(LABEL) else {
        log::debug!("trim: close_trim with no trim window open");
        return;
    };
    if exporting() {
        log::warn!("trim: ignoring close_trim while an export is in flight");
        return;
    }
    match window.close() {
        Ok(()) => log::info!("trim: window closed by the frontend"),
        Err(error) => log::warn!("trim: cannot close the trim window: {error}"),
    }
}

/// Ready handshake: the page registered its `trim-target` listener, so replay the last target.
pub fn report_ready(app: &AppHandle) -> Option<String> {
    TRIM_READY.store(true, Ordering::Relaxed);
    log::debug!("trim: page ready");
    // A panel that missed the creation event still learns the window is open from this handshake.
    emit_trim_state(app, true);
    TRIM_TARGET.lock().ok().and_then(|target| target.clone())
}

/// Whether the trim window currently exists (used by `get_editor_state`).
pub fn is_open(app: &AppHandle) -> bool {
    app.get_webview_window(LABEL).is_some()
}

/// Emits the panel's `trim-state` mirror; a failure is logged but never fails the command.
fn emit_trim_state(app: &AppHandle, open: bool) {
    if let Err(error) = app.emit(STATE_EVENT, TrimStateEvent { open }) {
        log::warn!("trim: cannot emit {STATE_EVENT}: {error}");
    }
}

/// Clears the per-window bookkeeping when the window is destroyed.
fn reset_trim_state() {
    TRIM_READY.store(false, Ordering::Relaxed);
    if let Ok(mut target) = TRIM_TARGET.lock() {
        *target = None;
    }
}

/// Emits one coarse progress milestone (0–1).
pub(crate) fn emit_progress(app: &AppHandle, id: &str, progress: f64) {
    if let Err(error) = app.emit(
        PROGRESS_EVENT,
        TrimProgressEvent {
            id: id.to_string(),
            progress,
        },
    ) {
        log::warn!("trim: cannot emit {PROGRESS_EVENT}: {error}");
    }
}

/// Same-directory temp path for the atomic replace (hidden, unique, original extension).
fn temp_path(target: &Path) -> PathBuf {
    let dir = target
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let extension = target
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| format!(".{extension}"))
        // The renderer derives the container from the extension; recordings are mp4/mov, and a
        // missing/opaque extension still gets a usable mp4 container.
        .unwrap_or_else(|| ".mp4".to_string());
    let mut hasher = blake3::Hasher::new();
    hasher.update(&item::now_ms().to_le_bytes());
    hasher.update(&std::process::id().to_le_bytes());
    hasher.update(&TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed).to_le_bytes());
    hasher.update(target.as_os_str().to_string_lossy().as_bytes());
    let token = hasher.finalize().to_hex();
    dir.join(format!("{TEMP_PREFIX}{}{extension}", &token[..32]))
}

/// Builds a validated [`TrimPlan`] for one item with explicit roots (unit-testable).
///
/// The ownership check lives here even though `trim` re-resolves the item: a recording outside
/// `recordings_root` must never be replaced, and the check must stay in front of every write.
pub fn plan_trim(
    item: &Item,
    size_bytes: u64,
    duration_ms: u64,
    start_ms: i64,
    end_ms: i64,
    recordings_root: &Path,
) -> Result<TrimPlan, TrimError> {
    if item.kind != ItemKind::Recording {
        return Err(TrimError::NotRecording(item.id.clone()));
    }
    if !item.path.is_file() {
        return Err(TrimError::FileMissing(item.path.clone()));
    }
    if !path_is_inside(&item.path, recordings_root) {
        return Err(TrimError::NotOwned(item.path.clone()));
    }
    let range = validate_trim(size_bytes, duration_ms, start_ms, end_ms)?;
    Ok(TrimPlan {
        target: item.path.clone(),
        temp: temp_path(&item.path),
        range,
    })
}

/// Removes a render temp file unless the export disarmed the guard after a successful rename.
struct TempGuard {
    path: PathBuf,
    armed: bool,
}

impl TempGuard {
    fn new(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
            armed: true,
        }
    }

    /// Keeps the file (the successful rename already moved it over the original).
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for TempGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        match fs::remove_file(&self.path) {
            Ok(()) => log::info!("trim: removed temp file {}", self.path.display()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => log::warn!(
                "trim: cannot remove temp file {}: {error}",
                self.path.display()
            ),
        }
    }
}

/// Whether an export currently owns the single trim window.
///
/// Used by [`open`] (refuse a retarget), [`close`] and the window's close-request handler.
pub fn exporting() -> bool {
    EXPORTING.load(Ordering::SeqCst)
}

/// Serializes exports; the flag clears on every return path (including a panic unwind).
struct ExportGuard;

impl ExportGuard {
    fn begin() -> anyhow::Result<Self> {
        if EXPORTING.swap(true, Ordering::SeqCst) {
            bail!("trim: another export is already running");
        }
        Ok(Self)
    }
}

impl Drop for ExportGuard {
    fn drop(&mut self) {
        EXPORTING.store(false, Ordering::SeqCst);
    }
}

/// Whether a render may replace `original`: the item must still be on the line and still point at
/// the same file.
///
/// A take-down or drag-out during the (minutes-long for large recordings) render must drop the
/// rendered bytes instead of replacing a file no card refers to anymore.
fn replace_is_allowed(current: Option<&Item>, original: &Item) -> bool {
    current.is_some_and(|current| current.id == original.id && current.path == original.path)
}

/// Streams the rendered file's hash and atomically moves it over the original, returning the hash.
///
/// On success the temp no longer exists (the rename consumed it); on failure it is left in place so
/// the caller's [`TempGuard`] removes it. [`replace_is_allowed`] must pass before this runs.
fn commit_rendered(temp: &Path, target: &Path) -> anyhow::Result<u64> {
    let hash = dedupe::hash_file(temp).with_context(|| {
        format!(
            "trim: cannot hash the rendered file {}",
            temp.display()
        )
    })?;
    fs::rename(temp, target).with_context(|| {
        format!(
            "trim: cannot replace {} with {}",
            target.display(),
            temp.display()
        )
    })?;
    Ok(hash)
}

/// Records a replacement that already happened on disk and returns the item now on the line.
///
/// The atomic rename committed the export, so a persistence failure is returned as a warning (the
/// caller logs it) instead of an error: returning `Err` would invite a retry that cannot undo the
/// replace and would trim the already-trimmed file again. `replace_item` mutates memory before it
/// persists, so the in-memory row carries the new metadata either way.
fn publish_replaced(state: &AppState, updated: Item) -> (Item, Option<String>) {
    match state.replace_item(updated.clone()) {
        Ok(_) => (state.item(&updated.id).unwrap_or(updated), None),
        Err(error) => (
            state.item(&updated.id).unwrap_or(updated),
            Some(format!("{error:#}")),
        ),
    }
}

/// Trims item `id` to `[start_ms, end_ms)`, atomically replaces the original and refreshes metadata.
///
/// Blocks on WinRT and disk I/O; the `trim_video` command runs it on a `spawn_blocking` worker. On
/// every failure the temp file is removed and the original is left untouched. Only one export can
/// run at a time ([`ExportGuard`]), and the item is re-checked immediately before the rename so a
/// take-down during the render cannot replace an untracked file.
pub fn trim(app: &AppHandle, id: &str, start_ms: i64, end_ms: i64) -> anyhow::Result<Item> {
    let state = app
        .try_state::<AppState>()
        .ok_or_else(|| anyhow::anyhow!("trim: the app state is not managed"))?;
    let item = state
        .item(id)
        .ok_or_else(|| TrimError::NotFound(id.to_string()))?;
    let recordings = recordings_dir()
        .ok_or_else(|| anyhow::anyhow!("trim: USERPROFILE is not set; cannot locate recordings"))?;
    let size_bytes = fs::metadata(&item.path)
        .map_err(|_| TrimError::FileMissing(item.path.clone()))?
        .len();
    // Size and range sanity never need the media stack: refuse an impossible request before the
    // (blocking) WinRT duration probe, then apply the duration-dependent checks in `plan_trim`.
    validate_preflight(size_bytes, start_ms, end_ms)?;
    let duration_ms = video::duration_ms(&item.path)?;
    let plan = plan_trim(&item, size_bytes, duration_ms, start_ms, end_ms, &recordings)?;
    let _export = ExportGuard::begin()?;
    log::info!(
        "trim: exporting {} [{}, {}) of {} ms into {}",
        plan.target.display(),
        plan.range.start_ms,
        plan.range.end_ms,
        duration_ms,
        plan.temp.display()
    );

    emit_progress(app, id, 0.05);
    // The guard removes the render destination on every early return below (hash/rename/render
    // failure); it is disarmed only once the temp became the original.
    let mut temp = TempGuard::new(&plan.temp);
    render_to_temp(&plan.target, &plan.temp, plan.range, duration_ms)?;
    emit_progress(app, id, 0.6);

    // The render can take minutes for a large clip; re-check the line after it so a take-down or a
    // drag-out cannot make this export replace a file whose card is gone.
    if !replace_is_allowed(state.item(id).as_ref(), &item) {
        return Err(TrimError::NotFound(id.to_string()).into());
    }
    let hash = commit_rendered(&plan.temp, &plan.target)?;
    temp.disarm();
    emit_progress(app, id, 1.0);
    log::info!(
        "trim: replaced {} ({} ms, hash {hash:#018x})",
        plan.target.display(),
        plan.range.duration_ms()
    );

    // Refresh the first-frame preview best-effort: the file is already replaced, so a decode
    // failure must not turn a successful (and irreversible) export into an error. Dropping the old
    // `thumbPath` first means a failed refresh falls back to the tile + play badge instead of
    // keeping pre-trim bytes behind the new `updatedAt` version.
    let mut updated = item.clone();
    updated.thumb_path = None;
    match (video::first_frame(&updated.path, thumbnails::THUMB_MAX_EDGE), thumbnails::cache_dir()) {
        (Ok(info), Ok(cache)) => {
            log::info!(
                "trim: refreshed the recording preview from a {} ms clip ({}x{})",
                info.duration_ms,
                info.width,
                info.height
            );
            if !thumbnails::attach_recording_preview(&mut updated, &cache, &info) {
                log::warn!(
                    "trim: cannot write the refreshed recording preview for {}",
                    updated.path.display()
                );
            }
        }
        (Ok(_), Err(error)) => {
            log::warn!("trim: thumbnail cache unavailable after export: {error:#}");
        }
        (Err(error), _) => {
            log::warn!(
                "trim: cannot refresh the recording preview for {}: {error:#}",
                updated.path.display()
            );
        }
    }
    updated.hash = hash;
    // Monotonic: two edits inside the same millisecond must still produce a new `?v=` URL.
    updated.updated_at = item::now_ms().max(updated.updated_at.saturating_add(1));

    // The file is already the trimmed render, so a persistence failure is a warning: an error would
    // let the UI retry an export that cannot succeed and would trim the trimmed file again.
    let (current, warning) = publish_replaced(&state, updated);
    if let Some(warning) = warning {
        log::warn!("trim: the file was replaced but its metadata could not be persisted: {warning}");
    }
    // Publish whatever is current so the panel cannot keep a card that disagrees with the file that
    // was just replaced (on a persistence failure `replace_item` already mutated memory).
    if app_state::emit_state(app).is_none() {
        log::warn!("trim: exported but the state event could not be emitted");
    }
    Ok(current)
}

/// Renders `[range]` of `source` to `temp` with WinRT `MediaComposition` (worker thread only).
///
/// The function is the testable half of the export: it leaves `temp` in place on success and does
/// not clean up on failure (the caller's [`TempGuard`] or [`render_checked`] owns that policy).
fn render_to_temp(
    source: &Path,
    temp: &Path,
    range: TrimRange,
    duration_ms: u64,
) -> anyhow::Result<()> {
    let _apartment = Apartment::init()?;
    let clip = video::load_clip(source)?;
    let composition = MediaComposition::new()
        .map_err(|error| anyhow::anyhow!("trim: cannot activate MediaComposition: {error}"))?;
    composition
        .Clips()
        .and_then(|clips| clips.Append(&clip))
        .map_err(|error| anyhow::anyhow!("trim: cannot add the clip to the composition: {error}"))?;

    // WinRT TimeSpan is 100 ns ticks; the cap (60 min) keeps this far inside i64.
    const TICKS_PER_MS: i64 = 10_000;
    let start = TimeSpan {
        Duration: i64::try_from(range.start_ms)
            .unwrap_or(i64::MAX)
            .saturating_mul(TICKS_PER_MS),
    };
    let trim_from_end_ms = duration_ms.saturating_sub(range.end_ms);
    let end = TimeSpan {
        Duration: i64::try_from(trim_from_end_ms)
            .unwrap_or(i64::MAX)
            .saturating_mul(TICKS_PER_MS),
    };
    clip.SetTrimTimeFromStart(start)
        .map_err(|error| anyhow::anyhow!("trim: cannot set the start trim point: {error}"))?;
    clip.SetTrimTimeFromEnd(end)
        .map_err(|error| anyhow::anyhow!("trim: cannot set the end trim point: {error}"))?;

    let destination = create_render_destination(temp)?;
    let reason = composition
        .RenderToFileAsync(&destination)
        .map_err(|error| anyhow::anyhow!("trim: cannot start the render: {error}"))?
        .join()
        .map_err(|error| {
            anyhow::anyhow!(
                "trim: the media runtime could not render the recording \
                 (unsupported codec or DRM?): {error}"
            )
        })?;
    if reason.0 != 0 {
        bail!("trim: the media runtime reported transcode failure {reason:?}");
    }
    let metadata = fs::metadata(temp)
        .with_context(|| format!("trim: the rendered file {} is missing", temp.display()))?;
    if metadata.len() == 0 {
        bail!("trim: the rendered file {} is empty", temp.display());
    }
    Ok(())
}

/// Test helper: renders into `temp` and removes it when the render fails.
#[cfg(test)]
fn render_checked(
    source: &Path,
    temp: &Path,
    range: TrimRange,
    duration_ms: u64,
) -> anyhow::Result<()> {
    let mut guard = TempGuard::new(temp);
    render_to_temp(source, temp, range, duration_ms)?;
    guard.disarm();
    Ok(())
}

/// Creates (or replaces) the `StorageFile` the composition renders into.
///
/// `RenderToFileAsync` needs a `StorageFile`, which cannot be obtained for a path that does not
/// exist; creating it through `StorageFolder` is the documented WinRT pattern and also works for
/// the hidden dot-file name. `ReplaceExisting` keeps the deterministic temp name.
fn create_render_destination(path: &Path) -> anyhow::Result<StorageFile> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| anyhow::anyhow!("trim: the render destination has no parent: {}", path.display()))?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            anyhow::anyhow!("trim: the render destination has no usable file name: {}", path.display())
        })?;
    let folder_path = video::wide_path(parent)?;
    let folder = StorageFolder::GetFolderFromPathAsync(&folder_path)
        .map_err(|error| anyhow::anyhow!("trim: cannot open the render folder: {error}"))?
        .join()
        .map_err(|error| anyhow::anyhow!("trim: cannot read the render folder: {error}"))?;
    let name: HSTRING = HSTRING::from(name);
    folder
        .CreateFileAsync(&name, CreationCollisionOption::ReplaceExisting)
        .map_err(|error| anyhow::anyhow!("trim: cannot create the render file: {error}"))?
        .join()
        .map_err(|error| anyhow::anyhow!("trim: cannot create the render file: {error}"))
}

/// Removes stale export temp files from `dir` and returns the number removed.
///
/// Mirrors the markup sweep: only names this module creates (`.hangline-trim-*`) and only files
/// older than [`STALE_TEMP_AGE`], so a concurrent export is never disturbed.
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
        if !name.starts_with(TEMP_PREFIX) {
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
        log::info!(
            "trim: swept {removed} stale trim temp file(s) from {}",
            dir.display()
        );
    }
    removed
}

/// Startup sweep of the recordings folder (called from `lib::run` after the state is managed).
pub fn sweep_stale_temp_files() {
    let Some(dir) = recordings_dir() else {
        return;
    };
    sweep_stale_temp_files_in(&dir);
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::time::SystemTime;

    const TEST_ID: &str = "0123456789abcdef0123456789abcdef";

    /// Scratch directory under `target/` (git-ignored); never touches `%USERPROFILE%`.
    fn test_dir(tag: &str) -> PathBuf {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("trim-tests")
            .join(format!("{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create the trim test dir");
        dir
    }

    fn item_at(path: &Path, kind: ItemKind) -> Item {
        Item {
            id: TEST_ID.to_string(),
            path: path.to_path_buf(),
            thumb_path: None,
            kind,
            hash: 7,
            width: 640,
            height: 360,
            created_at: 1,
            added_at: 2,
            updated_at: 0,
        }
    }

    #[test]
    fn window_url_carries_the_id_theme_and_language() {
        let url = trim_url(TEST_ID, "dark", "zh-Hans");
        assert!(url.starts_with("index.html?"), "url: {url}");
        assert!(url.contains("window=trim"), "url: {url}");
        assert!(url.contains(&format!("id={TEST_ID}")), "url: {url}");
        assert!(url.contains("theme=dark"), "url: {url}");
        assert!(url.contains("lang=zh-Hans"), "url: {url}");
    }

    #[test]
    fn window_size_fits_the_video_aspect_into_900x600() {
        // The P3-5 fixture (640×360): 16:9 fitted into 900 wide.
        assert_eq!(trim_window_size(640, 360), (900.0, 506.0));
        // 1080p has the same aspect; the 900×600 cap decides before the work area.
        assert_eq!(trim_window_size(1920, 1080), (900.0, 506.0));
        // 4:3 fits the height cap exactly (width follows the source aspect, 800 px).
        assert_eq!(trim_window_size(800, 600), (800.0, 600.0));
        // Portrait: height-capped, then the 640 px width minimum wins (aspect intentionally
        // broken; the page contains the video with `object-fit: contain`).
        assert_eq!(trim_window_size(360, 640), (640.0, 600.0));
        // Square: height-capped at 600, width clamped to the 640 minimum.
        assert_eq!(trim_window_size(100, 100), (640.0, 600.0));
        // Unknown dimensions open at the minimum usable window instead of failing.
        assert_eq!(trim_window_size(0, 0), (640.0, 400.0));
        assert_eq!(trim_window_size(640, 0), (640.0, 400.0));
    }

    #[test]
    fn validation_accepts_the_boundaries() {
        // Exactly 2 GiB, exactly 60 min, exactly `duration` as the end and 101 ms of selection.
        assert_eq!(
            validate_trim(MAX_TRIM_BYTES, MAX_TRIM_DURATION_MS, 0, 101),
            Ok(TrimRange {
                start_ms: 0,
                end_ms: 101
            })
        );
        assert_eq!(
            validate_trim(1, 3000, 1000, 2500),
            Ok(TrimRange {
                start_ms: 1000,
                end_ms: 2500
            })
        );
        // The end may equal the duration exactly.
        assert_eq!(
            validate_trim(1, 3000, 0, 3000),
            Ok(TrimRange {
                start_ms: 0,
                end_ms: 3000
            })
        );
    }

    #[test]
    fn validation_rejects_files_over_the_caps() {
        assert_eq!(
            validate_trim(MAX_TRIM_BYTES + 1, 3000, 0, 1000),
            Err(TrimError::TooLarge {
                size_bytes: MAX_TRIM_BYTES + 1
            })
        );
        assert_eq!(
            validate_trim(1, MAX_TRIM_DURATION_MS + 1, 0, 1000),
            Err(TrimError::TooLong {
                duration_ms: MAX_TRIM_DURATION_MS + 1
            })
        );
        // The size limit is checked before the range: an oversized file is refused regardless.
        assert_eq!(
            validate_trim(MAX_TRIM_BYTES + 1, 3000, -5, -1),
            Err(TrimError::TooLarge {
                size_bytes: MAX_TRIM_BYTES + 1
            })
        );
    }

    #[test]
    fn validation_rejects_negative_unordered_and_short_ranges() {
        for (start, end) in [(-1, 1000), (0, -1), (1000, 1000), (1500, 1000), (0, 100)] {
            assert_eq!(
                validate_trim(1, 3000, start, end),
                Err(TrimError::InvalidRange {
                    start_ms: start,
                    end_ms: end
                }),
                "range {start}–{end} must be rejected"
            );
        }
        // One millisecond over the minimum is valid.
        assert!(validate_trim(1, 3000, 0, 101).is_ok());
    }

    #[test]
    fn validation_rejects_ranges_past_the_clip() {
        assert_eq!(
            validate_trim(1, 3000, 2900, 3500),
            Err(TrimError::RangeOutOfBounds {
                end_ms: 3500,
                duration_ms: 3000
            })
        );
        // A start past the duration also ends past it, so the same variant is reported.
        assert_eq!(
            validate_trim(1, 3000, 4000, 4200),
            Err(TrimError::RangeOutOfBounds {
                end_ms: 4200,
                duration_ms: 3000
            })
        );
        // A zero-length clip cannot satisfy the 100 ms rule.
        assert!(validate_trim(1, 0, 0, 1000).is_err());
    }

    #[test]
    fn plan_requires_a_recording_inside_the_recordings_root() {
        let root = test_dir("plan-roots");
        let captures = root.join("Captures");
        fs::create_dir_all(&captures).expect("create the captures root");
        let inside = captures.join("clip.mp4");
        fs::write(&inside, b"not really an mp4").expect("write the inside file");
        let outside = root.join("elsewhere.mp4");
        fs::write(&outside, b"not really an mp4").expect("write the outside file");

        let error = plan_trim(
            &item_at(&inside, ItemKind::Capture),
            10,
            3000,
            0,
            1000,
            &captures,
        )
        .expect_err("a non-recording item must be rejected");
        assert!(matches!(error, TrimError::NotRecording(_)), "{error}");

        let error = plan_trim(
            &item_at(&outside, ItemKind::Recording),
            10,
            3000,
            0,
            1000,
            &captures,
        )
        .expect_err("a file outside the recordings folder must be rejected");
        assert!(matches!(error, TrimError::NotOwned(_)), "{error}");

        let error = plan_trim(
            &item_at(&captures.join("missing.mp4"), ItemKind::Recording),
            10,
            3000,
            0,
            1000,
            &captures,
        )
        .expect_err("a missing file must be rejected");
        assert!(matches!(error, TrimError::FileMissing(_)), "{error}");

        let plan = plan_trim(
            &item_at(&inside, ItemKind::Recording),
            10,
            3000,
            1000,
            2500,
            &captures,
        )
        .expect("an owned recording plans");
        assert_eq!(plan.target, inside);
        assert_eq!((plan.range.start_ms, plan.range.end_ms), (1000, 2500));
        assert_eq!(plan.range.duration_ms(), 1500);
        assert_eq!(
            plan.temp.parent(),
            Some(captures.as_path()),
            "the temp must be a sibling of the original (same volume → atomic rename)"
        );
        let temp_name = plan
            .temp
            .file_name()
            .and_then(|name| name.to_str())
            .expect("temp name");
        assert!(temp_name.starts_with(".hangline-trim-"), "{temp_name}");
        assert!(temp_name.ends_with(".mp4"), "{temp_name}");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn temp_paths_are_unique_hidden_and_keep_the_original_extension() {
        let dir = test_dir("temp-paths");
        let target = dir.join("Game Bar Clip.MOV");
        fs::write(&target, b"video").expect("write the target");
        let first = temp_path(&target);
        let second = temp_path(&target);
        assert_ne!(first, second, "two exports must never share a temp file");
        assert_eq!(first.parent(), Some(dir.as_path()));
        let name = first.file_name().and_then(|name| name.to_str()).expect("name");
        assert!(name.starts_with(".hangline-trim-"), "{name}");
        assert!(name.ends_with(".MOV"), "the case of the extension is preserved: {name}");
        // A dot-file is invisible to the folder watcher (`is_candidate` rejects it).
        assert!(!crate::watchers::dirs::is_candidate(
            &first,
            crate::watchers::dirs::Opts::recordings()
        ));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn event_payloads_match_the_frontend_contract() {
        assert_eq!(
            serde_json::to_value(TrimStateEvent { open: true }).expect("serialize"),
            serde_json::json!({ "open": true })
        );
        assert_eq!(
            serde_json::to_value(TrimProgressEvent {
                id: TEST_ID.to_string(),
                progress: 0.6
            })
            .expect("serialize"),
            serde_json::json!({ "id": TEST_ID, "progress": 0.6 })
        );
    }

    #[test]
    fn startup_sweep_removes_only_stale_trim_temps() {
        let dir = test_dir("temp-sweep");
        let stale = dir.join(".hangline-trim-deadbeef.mp4");
        let fresh = dir.join(".hangline-trim-feedface.mp4");
        let recording = dir.join("keep.mp4");
        let other = dir.join("keep.tmp");
        fs::write(&stale, b"stale").expect("write the stale temp");
        fs::write(&fresh, b"fresh").expect("write the fresh temp");
        fs::write(&recording, b"video").expect("write the recording");
        fs::write(&other, b"tmp").expect("write the other temp");
        let old = SystemTime::now() - Duration::from_secs(2 * 60 * 60);
        fs::OpenOptions::new()
            .write(true)
            .open(&stale)
            .expect("open the stale temp")
            .set_modified(old)
            .expect("age the stale temp");

        assert_eq!(sweep_stale_temp_files_in(&dir), 1);
        assert!(!stale.exists(), "old trim residue must be swept");
        assert!(fresh.exists(), "a fresh temp may still be an in-flight render");
        assert!(recording.exists(), "the sweep must not touch recordings");
        assert!(other.exists(), "the sweep must not touch unrelated temp files");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn preflight_refuses_size_and_range_before_the_duration_probe() {
        assert!(matches!(
            validate_preflight(MAX_TRIM_BYTES + 1, 0, 1000),
            Err(TrimError::TooLarge { .. })
        ));
        // Range sanity is pure too: a negative, unordered or sub-minimum selection is refused
        // without a clip duration.
        for (start, end) in [(-1, 1000), (0, -1), (1000, 1000), (0, 100)] {
            assert!(
                matches!(
                    validate_preflight(1, start, end),
                    Err(TrimError::InvalidRange { .. })
                ),
                "range {start}–{end} must fail preflight"
            );
        }
        // A range that is only invalid because of the clip length passes preflight: the probe has
        // to report the duration before `validate_trim` can reject it.
        assert_eq!(validate_preflight(1, 0, 60_000_000), Ok(()));
        assert_eq!(validate_preflight(1, 0, 101), Ok(()));
    }

    #[test]
    fn temp_guard_removes_the_render_unless_disarmed() {
        let dir = test_dir("temp-guard");
        let temp = dir.join(".hangline-trim-guard.mp4");
        fs::write(&temp, b"rendered").expect("write the temp");
        drop(TempGuard::new(&temp));
        assert!(!temp.exists(), "an armed guard removes the rendered bytes");

        fs::write(&temp, b"kept").expect("write the temp again");
        let mut guard = TempGuard::new(&temp);
        guard.disarm();
        drop(guard);
        assert!(
            temp.exists(),
            "a disarmed guard keeps the file that became the original"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn replace_is_allowed_requires_the_same_live_item() {
        let dir = test_dir("replace-allowed");
        let path = dir.join("clip.mp4");
        fs::write(&path, b"video").expect("write the clip");
        let original = item_at(&path, ItemKind::Recording);
        assert!(!replace_is_allowed(None, &original), "a taken-down item");
        let mut moved = original.clone();
        moved.path = dir.join("other.mp4");
        assert!(!replace_is_allowed(Some(&moved), &original), "a replaced file");
        assert!(replace_is_allowed(Some(&original), &original));
        let _ = fs::remove_dir_all(dir);
    }

    /// P3-6 review follow-up: once the rename happened the export is committed, so a metadata
    /// persistence failure must be a warning (the item is still returned) instead of an error that
    /// would suggest a retry.
    #[test]
    fn publish_replaced_reports_a_metadata_failure_as_a_warning() {
        let dir = test_dir("publish-replaced");
        let source = dir.join("clip.mp4");
        fs::write(&source, b"video").expect("write the clip");
        let state = AppState::load(dir.clone());
        let original = item_at(&source, ItemKind::Recording);
        state.add_item(original.clone()).expect("add the item");
        // `save_atomic` writes `state.json.tmp` before renaming it; a directory in its place makes
        // the persistence step fail deterministically (a disk error with the same effect).
        fs::create_dir(dir.join("state.json.tmp")).expect("block the state temp path");

        let mut updated = original.clone();
        updated.hash = 99;
        updated.updated_at = 42;
        let (current, warning) = publish_replaced(&state, updated);
        let warning = warning.expect("a persistence failure must be reported as a warning");
        assert!(!warning.is_empty());
        assert_eq!(current.id, original.id);
        assert_eq!(current.hash, 99, "the in-memory row carries the new metadata");
        assert_eq!(state.snapshot().items[0].hash, 99);
        // The on-disk list lags until the next successful save; the next startup keeps the old row
        // (the file is already trimmed, so this is a metadata-only inconsistency).
        let on_disk: Vec<Item> =
            serde_json::from_slice(&fs::read(dir.join("state.json")).expect("read state.json"))
                .expect("state.json is a list");
        assert_eq!(on_disk[0].hash, original.hash);

        fs::remove_dir(dir.join("state.json.tmp")).expect("unblock the state temp path");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn export_guard_is_exclusive_and_clears_on_drop() {
        assert!(!exporting(), "the shared export flag must start clear");
        let first = ExportGuard::begin().expect("the first export starts");
        assert!(exporting());
        assert!(
            ExportGuard::begin().is_err(),
            "a concurrent export must be refused"
        );
        drop(first);
        assert!(!exporting(), "dropping the guard clears the flag");
        let second = ExportGuard::begin().expect("a later export is allowed");
        drop(second);
        assert!(!exporting());
    }

    /// P3-7 runtime evidence: render 1.0–2.5 s of the 3 s fixture, then verify the output duration
    /// with the same WinRT metadata the export used, plus the first-frame preview path.
    #[test]
    #[ignore = "needs HANG_TEST_VIDEO=<a real mp4>; see the module docs"]
    fn trims_a_real_video_to_the_requested_range() {
        let Some(raw) = std::env::var_os("HANG_TEST_VIDEO") else {
            panic!(
                "set HANG_TEST_VIDEO to a real 3 s 640x360 mp4 (see scripts/p3/fixture-video.ps1)"
            );
        };
        let dir = test_dir("real-trim");
        let source = dir.join("fixture.mp4");
        fs::copy(Path::new(&raw), &source).expect("copy the fixture");
        let original_hash = dedupe::hash_file(&source).expect("hash the original");
        let original_bytes = fs::read(&source).expect("read the original");

        let duration = video::duration_ms(&source).expect("duration of the fixture");
        assert!(
            (2850..=3150).contains(&duration),
            "the 3 s fixture must report ≈3000 ms, got {duration}"
        );
        let temp = temp_path(&source);
        let range = validate_trim(original_bytes.len() as u64, duration, 1000, 2500)
            .expect("the 1.0–2.5 s range is valid");
        render_checked(&source, &temp, range, duration).expect("render the trimmed clip");

        assert!(temp.is_file(), "the render must produce the temp file");
        let trimmed_ms = video::duration_ms(&temp).expect("duration of the trimmed clip");
        println!(
            "trim runtime: source {duration} ms, 1000–2500 ms selected, rendered {trimmed_ms} ms, \
             {} bytes",
            fs::metadata(&temp).expect("stat the render").len()
        );
        assert!(
            (1300..=1700).contains(&trimmed_ms),
            "1.0–2.5 s of a 3 s clip must be ≈1500 ms, got {trimmed_ms}"
        );
        // The original is not touched by the render (the rename happens in `trim`).
        assert_eq!(
            dedupe::hash_file(&source).expect("re-hash the original"),
            original_hash,
            "rendering must leave the original byte-identical"
        );
        // The trimmed file is a valid video for the preview pipeline (the E2E re-checks it with
        // ffprobe and the panel thumbnail).
        let info = video::first_frame(&temp, 432).expect("decode the trimmed first frame");
        assert!(info.width > 0 && info.height > 0, "natural dimensions");
        assert!(
            (info.image.width().max(info.image.height())) <= 432,
            "preview must fit 432 px"
        );

        // Failure *after* a real render (P3-6 review follow-up): hold the source with
        // FILE_SHARE_READ. The media stack can still read it and the render succeeds, but the
        // atomic replace needs DELETE access on the source and fails with a sharing violation.
        // The armed TempGuard must then remove the render and the original must stay identical.
        {
            use std::os::windows::fs::OpenOptionsExt;

            let locked = dir.join("locked.mp4");
            fs::copy(&source, &locked).expect("copy the locked fixture");
            let locked_bytes = fs::read(&locked).expect("read the locked fixture");
            let locked_hash = dedupe::hash_file(&locked).expect("hash the locked fixture");
            // 1 = FILE_SHARE_READ: readers allowed, delete/rename denied.
            let _lock = fs::OpenOptions::new()
                .read(true)
                .share_mode(1)
                .open(&locked)
                .expect("lock the source with FILE_SHARE_READ");
            let locked_temp = temp_path(&locked);
            let guard = TempGuard::new(&locked_temp);
            render_to_temp(&locked, &locked_temp, range, duration)
                .expect("a read lock must not block the render");
            assert!(
                locked_temp.is_file(),
                "the render must have produced a temp before the replace fails"
            );
            let error = commit_rendered(&locked_temp, &locked)
                .expect_err("the atomic replace must fail while the source denies delete sharing");
            println!("trim runtime: locked-source replace failed as expected: {error:#}");
            drop(guard);
            assert!(
                !locked_temp.exists(),
                "the failed replace must remove the rendered temp: {}",
                locked_temp.display()
            );
            assert_eq!(
                dedupe::hash_file(&locked).expect("re-hash the locked fixture"),
                locked_hash,
                "a failed replace must leave the original bytes untouched"
            );
            assert_eq!(
                fs::read(&locked).expect("re-read the locked fixture"),
                locked_bytes
            );
        }

        // Failure path: a corrupt source must return Err, remove the temp, and keep the original.
        let corrupt = dir.join("corrupt.mp4");
        fs::write(&corrupt, b"not really an mp4").expect("write the corrupt file");
        let failed_temp = temp_path(&corrupt);
        let error = render_checked(&corrupt, &failed_temp, range, duration)
            .expect_err("a corrupt source must fail");
        assert!(!format!("{error:#}").is_empty());
        assert!(
            !failed_temp.exists(),
            "a failed render must remove its temp file: {}",
            failed_temp.display()
        );
        assert_eq!(
            fs::read(&source).expect("re-read the original"),
            original_bytes,
            "the failure path must leave the original byte-identical"
        );
        let _ = fs::remove_dir_all(dir);
    }
}
