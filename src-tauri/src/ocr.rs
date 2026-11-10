//! Local OCR index PoC (Wave 2 W2.5): `Windows.Media.Ocr` only — offline, no model bytes shipped.
//!
//! # Why this is a PoC
//!
//! The index is a plain JSON file next to `settings.json`; it is not wired into the search UI yet.
//! The point is to measure whether the *system* OCR is good enough for Chinese screenshots before
//! committing to an index format or a UI. Everything is local: `OcrEngine` comes from the Windows
//! language pack (0 bytes added to the installer) and nothing leaves the machine.
//!
//! # Pipeline
//!
//! `image` decodes the file (PNG/JPG; the search index only covers still images) → RGBA is resized
//! to a max edge of [`MAX_OCR_EDGE`] (OCR quality/latency balance) → BGRA8 bytes → `SoftwareBitmap`
//! → `OcrEngine::RecognizeAsync` → `lines` (the engine's line boundaries) plus the contiguous text.
//!
//! # Index file
//!
//! `%LOCALAPPDATA%\Hangline\ocr-index.json`:
//! `{ version, language, entries: { "<itemId>": { hash, text, indexedAt } } }`.
//! Writes are atomic (`ocr-index.json.tmp` + rename). A missing, corrupt or future-version file is
//! treated as empty with a warning — the index is a cache, so it must never break the app. Entries
//! are keyed by the item id and validated by `hash` (the item's content hash): a rewritten file
//! (`annotate`/`trim`) has a new hash and is re-indexed.
//!
//! # CLI probe
//!
//! [`run_probe`] implements `hangline.exe --ocr-probe <dir>`: one JSON line per image file, then a
//! summary line; the exit code is the number of failed files capped at 1. It is used by the
//! evaluation scripts to measure recognition on a fixed set of real screenshots without the GUI.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::Context;
use image::{imageops::FilterType, RgbaImage};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use windows::Globalization::Language;
use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::DataWriter;
use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
use windows::Win32::System::Threading::{
    GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL,
};
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};

use crate::app_state::AppState;
use crate::item::ItemKind;

/// Largest image edge handed to the OCR engine (downscale big screenshots for latency/quality).
pub const MAX_OCR_EDGE: u32 = 2560;

/// Index file name inside `%LOCALAPPDATA%\Hangline`.
pub const INDEX_FILE: &str = "ocr-index.json";

/// Schema version of [`OcrIndex`]; a different version is treated as an empty index.
pub const INDEX_VERSION: u32 = 1;

/// Characters of context kept on each side of a search hit (W2.5 excerpt contract).
pub const EXCERPT_RADIUS: usize = 40;

/// File extensions the index covers (case-insensitive); recordings are skipped explicitly.
const OCR_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg"];

/// Availability of the system OCR engine (command `ocr_status`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OcrAvailability {
    /// Whether an OCR engine could be created (a Windows OCR language pack is installed).
    pub available: bool,
    /// Language tag of the created engine (`"zh-Hans"`, ...), when available.
    pub language: Option<String>,
    /// Installed OCR recognizer languages (tags), for diagnostics and the evaluation report.
    pub languages: Vec<String>,
    /// Why the engine is unavailable; `None` when it could be created.
    pub error: Option<String>,
}

/// Plain text of one recognized image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OcrText {
    /// Contiguous text (`OcrResult::Text`, the engine's own spacing/line handling).
    pub text: String,
    /// One entry per OCR line, newline-free; used for the probe's `lines` count.
    pub lines: Vec<String>,
}

/// One persisted OCR index entry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OcrIndexEntry {
    /// Item content hash at indexing time; a different hash means the file was rewritten.
    pub hash: u64,
    /// Recognized text.
    pub text: String,
    /// When this row was recognized (milliseconds since the Unix epoch).
    pub indexed_at: i64,
}

/// The persisted OCR index (`%LOCALAPPDATA%\Hangline\ocr-index.json`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct OcrIndex {
    /// Schema version ([`INDEX_VERSION`]).
    pub version: u32,
    /// Engine language last used to build the index (informational; a change forces a rebuild).
    pub language: Option<String>,
    /// Recognized text by item id (BTreeMap: stable file order for diffs/tests).
    pub entries: BTreeMap<String, OcrIndexEntry>,
}

impl Default for OcrIndex {
    fn default() -> Self {
        Self {
            version: INDEX_VERSION,
            language: None,
            entries: BTreeMap::new(),
        }
    }
}

impl OcrIndex {
    /// Whether `id`/`hash` needs (re-)indexing; `force` rebuilds even an unchanged row.
    pub fn entry_needs_index(&self, id: &str, hash: u64, force: bool) -> bool {
        force || self.entries.get(id).map(|entry| entry.hash != hash).unwrap_or(true)
    }

    /// Inserts or replaces one entry (one row per item id).
    pub fn upsert(&mut self, id: &str, hash: u64, text: &str, indexed_at: i64) {
        self.entries.insert(
            id.to_string(),
            OcrIndexEntry {
                hash,
                text: text.to_string(),
                indexed_at,
            },
        );
    }

    /// Case-insensitive substring search; Chinese matches on the original characters.
    ///
    /// Returns one hit per matching item (first match only), in stable id order. An empty/blank
    /// query returns no hits.
    pub fn search(&self, query: &str) -> Vec<OcrHit> {
        let needle = query.trim().to_lowercase();
        if needle.is_empty() {
            return Vec::new();
        }
        self.entries
            .iter()
            .filter_map(|(id, entry)| {
                excerpt_for(&entry.text, &needle).map(|excerpt| OcrHit {
                    id: id.clone(),
                    excerpt,
                })
            })
            .collect()
    }
}

/// One search hit: the item id plus the context window around the first match.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OcrHit {
    /// Opaque item id (the frontend resolves it through its existing item store).
    pub id: String,
    /// At most [`EXCERPT_RADIUS`] characters before/after the match (clamped to the text).
    pub excerpt: String,
}

/// Reply of `build_ocr_index` (the command is the only caller).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OcrBuildReport {
    /// Items that were actually sent to the engine (skips and failures excluded).
    pub attempted: usize,
    /// Items newly written into the index (attempted minus failed).
    pub indexed: usize,
    /// Items already indexed with the same hash (no OCR work).
    pub skipped: usize,
    /// Items whose OCR failed (logged, never fatal).
    pub failed: usize,
    /// Wall-clock duration of the whole build.
    pub elapsed_ms: u64,
    /// Whether an OCR engine was available at all.
    pub available: bool,
}

// ---------------------------------------------------------------------------------------------
// WinRT plumbing
// ---------------------------------------------------------------------------------------------

/// Initializes the WinRT apartment for the calling thread; idempotent and thread-local.
///
/// OCR runs on worker threads (`build`) and on the CLI main thread (probe), neither of which is a
/// Tauri/apartment-initialized thread. A thread that is already in an apartment (STA on the GUI
/// main thread) returns `RPC_E_CHANGED_MODE`; WinRT activation still works there, so that case is
/// not an error. Any other failure is returned to the caller to report.
fn ensure_winrt() -> Result<(), String> {
    // SAFETY: RoInitialize has no pointer arguments; repeated calls on the same thread are valid
    // WinRT behavior (S_FALSE is smoothed into Ok by the windows crate).
    match unsafe { RoInitialize(RO_INIT_MULTITHREADED) } {
        Ok(()) => Ok(()),
        Err(error) if error.code() == RPC_E_CHANGED_MODE => Ok(()),
        Err(error) => Err(format!("cannot initialize WinRT: {error}")),
    }
}

/// Blocks the calling thread until a WinRT async operation completes.
///
/// The Windows async types implement `std::future::IntoFuture` (the `windows` crate already enables
/// `windows-future/std`), so a no-op waker plus a short poll loop is enough here: OCR calls always
/// run on a dedicated worker thread or the CLI probe thread, never on the UI thread, and the
/// operation does not need a real executor to make progress (its completion handler sets the
/// results; polling re-checks the WinRT status).
fn block_on_operation<F: std::future::IntoFuture>(operation: F) -> F::Output {
    use std::future::Future;
    use std::task::{Context, Poll, Waker};

    let mut future = Box::pin(operation.into_future());
    let mut context = Context::from_waker(Waker::noop());
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(output) => return output,
            Poll::Pending => std::thread::sleep(std::time::Duration::from_millis(2)),
        }
    }
}

/// Tags of the OCR recognizer languages installed on this machine (diagnostics list).
fn available_language_tags() -> Vec<String> {
    let Ok(languages) = OcrEngine::AvailableRecognizerLanguages() else {
        log::warn!("ocr: cannot enumerate the installed OCR recognizer languages");
        return Vec::new();
    };
    let count = languages.Size().unwrap_or(0);
    let mut tags = Vec::with_capacity(count as usize);
    for index in 0..count {
        if let Ok(language) = languages.GetAt(index) {
            if let Ok(tag) = language.LanguageTag() {
                tags.push(tag.to_string());
            }
        }
    }
    tags
}

/// Creates an OCR engine: the user's profile languages first, then the installed recognizer
/// languages with a Simplified-Chinese preference (the PoC targets Chinese screenshots).
fn create_engine() -> Result<(OcrEngine, String), String> {
    let mut error: Option<String>;
    match OcrEngine::TryCreateFromUserProfileLanguages() {
        Ok(engine) => {
            let language = engine
                .RecognizerLanguage()
                .ok()
                .and_then(|language| language.LanguageTag().ok())
                .map(|tag| tag.to_string())
                .unwrap_or_else(|| "unknown".to_string());
            log::info!("ocr: engine created from the user profile languages ({language})");
            return Ok((engine, language));
        }
        Err(cause) => {
            log::info!(
                "ocr: no engine for the user profile languages ({cause}); trying installed languages"
            );
            error = Some(cause.to_string());
        }
    }

    let tags = available_language_tags();
    let mut candidates: Vec<&String> = tags
        .iter()
        .filter(|tag| tag.to_ascii_lowercase().starts_with("zh"))
        .collect();
    candidates.extend(
        tags.iter()
            .filter(|tag| !tag.to_ascii_lowercase().starts_with("zh")),
    );
    for tag in candidates {
        let Ok(language) = Language::CreateLanguage(&windows::core::HSTRING::from(tag.as_str()))
        else {
            continue;
        };
        match OcrEngine::TryCreateFromLanguage(&language) {
            Ok(engine) => {
                log::info!("ocr: engine created from the installed language {tag}");
                return Ok((engine, tag.clone()));
            }
            Err(cause) => error = Some(cause.to_string()),
        }
    }

    Err(error.unwrap_or_else(|| {
        "no OCR recognizer language is installed (add a Windows language pack with OCR support)"
            .to_string()
    }))
}

/// Best-effort background-thread priority drop (OCR is a PoC batch job, never latency critical).
fn lower_thread_priority() {
    // SAFETY: GetCurrentThread returns a pseudo-handle for the calling thread; SetThreadPriority
    // only changes a scheduling hint and cannot invalidate memory.
    let result = unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL) };
    if let Err(error) = result {
        log::debug!("ocr: cannot lower the build thread priority: {error}");
    }
}

/// Reports the system OCR availability (engine + installed languages); never panics.
pub fn availability() -> OcrAvailability {
    let languages = if ensure_winrt().is_ok() {
        available_language_tags()
    } else {
        Vec::new()
    };
    match ensure_winrt() {
        Ok(()) => match create_engine() {
            Ok((_engine, language)) => OcrAvailability {
                available: true,
                language: Some(language),
                languages,
                error: None,
            },
            Err(error) => {
                log::warn!("ocr: engine unavailable: {error}");
                OcrAvailability {
                    available: false,
                    language: None,
                    languages,
                    error: Some(error),
                }
            }
        },
        Err(error) => {
            log::warn!("ocr: {error}");
            OcrAvailability {
                available: false,
                language: None,
                languages,
                error: Some(error),
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Recognition
// ---------------------------------------------------------------------------------------------

/// Dimensions to feed the OCR engine: `(width, height)` scaled down so the longest edge is at most
/// `max_edge`; never upscales. Pure (unit-tested with the landscape/portrait/square boundaries).
pub fn target_dimensions(width: u32, height: u32, max_edge: u32) -> (u32, u32) {
    let longest = width.max(height);
    if longest == 0 || longest <= max_edge {
        return (width.max(1), height.max(1));
    }
    let scale = f64::from(max_edge) / f64::from(longest);
    let scaled_width = ((f64::from(width) * scale).round() as u32).max(1);
    let scaled_height = ((f64::from(height) * scale).round() as u32).max(1);
    (scaled_width.min(max_edge), scaled_height.min(max_edge))
}

/// Whether the OCR index covers this file (still images only; recordings are skipped by kind too).
pub fn is_supported_image(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| {
            OCR_EXTENSIONS
                .iter()
                .any(|supported| extension.eq_ignore_ascii_case(supported))
        })
        .unwrap_or(false)
}

/// Decodes `path` and runs the system OCR engine on it.
pub fn recognize_file(path: &Path) -> Result<OcrText, String> {
    ensure_winrt()?;
    let image = image::ImageReader::open(path)
        .map_err(|error| format!("cannot open image {}: {error}", path.display()))?
        .with_guessed_format()
        .map_err(|error| format!("cannot sniff image format of {}: {error}", path.display()))?
        .decode()
        .map_err(|error| format!("cannot decode image {}: {error}", path.display()))?
        .to_rgba8();
    recognize_rgba(&image)
}

/// Runs OCR on one decoded RGBA image (shared by the file path and the real-OCR test).
fn recognize_rgba(image: &RgbaImage) -> Result<OcrText, String> {
    let (engine, _language) = create_engine()?;
    let (width, height) = target_dimensions(image.width(), image.height(), MAX_OCR_EDGE);
    let prepared = if (width, height) != (image.width(), image.height()) {
        log::debug!(
            "ocr: downscaling {}x{} to {width}x{height} for recognition",
            image.width(),
            image.height()
        );
        image::imageops::resize(image, width, height, FilterType::Triangle)
    } else {
        image.clone()
    };

    // `SoftwareBitmap` takes BGRA8; screenshots are opaque so alpha stays 255.
    let mut bgra = Vec::with_capacity((prepared.width() as usize) * (prepared.height() as usize) * 4);
    for pixel in prepared.pixels() {
        let [red, green, blue, alpha] = pixel.0;
        bgra.extend_from_slice(&[blue, green, red, alpha]);
    }
    let writer = DataWriter::new().map_err(|error| format!("cannot create the OCR buffer: {error}"))?;
    writer
        .WriteBytes(&bgra)
        .map_err(|error| format!("cannot fill the OCR buffer: {error}"))?;
    let buffer = writer
        .DetachBuffer()
        .map_err(|error| format!("cannot detach the OCR buffer: {error}"))?;
    let bitmap = SoftwareBitmap::CreateCopyFromBuffer(
        &buffer,
        BitmapPixelFormat::Bgra8,
        prepared.width() as i32,
        prepared.height() as i32,
    )
    .map_err(|error| format!("cannot build the OCR bitmap: {error}"))?;

    let operation = engine
        .RecognizeAsync(&bitmap)
        .map_err(|error| format!("cannot start the OCR recognition: {error}"))?;
    let result = block_on_operation(operation)
        .map_err(|error| format!("OCR recognition failed: {error}"))?;

    let mut lines = Vec::new();
    if let Ok(line_view) = result.Lines() {
        for index in 0..line_view.Size().unwrap_or(0) {
            if let Ok(line) = line_view.GetAt(index) {
                if let Ok(text) = line.Text() {
                    lines.push(text.to_string());
                }
            }
        }
    }
    let text = result
        .Text()
        .map(|text| text.to_string())
        .unwrap_or_else(|_| lines.join("\n"));
    Ok(OcrText { text, lines })
}

// ---------------------------------------------------------------------------------------------
// Index persistence
// ---------------------------------------------------------------------------------------------

/// Full path of the OCR index file (`%LOCALAPPDATA%\Hangline\ocr-index.json`).
pub fn index_path() -> anyhow::Result<PathBuf> {
    Ok(crate::settings::data_dir()?.join(INDEX_FILE))
}

/// Parses index bytes; any corruption/unknown version is treated as an empty index with a warning.
///
/// The OCR index is a cache: it must never make the app (or a search) fail, so a broken file is
/// simply rebuilt by the next `build_ocr_index`. Pure, so the tolerance is unit-tested.
pub fn parse_index(bytes: &[u8]) -> OcrIndex {
    match serde_json::from_slice::<OcrIndex>(bytes) {
        Ok(index) if index.version == INDEX_VERSION => index,
        Ok(index) => {
            log::warn!(
                "ocr: index version {} is not supported (expected {INDEX_VERSION}); rebuilding",
                index.version
            );
            OcrIndex::default()
        }
        Err(error) => {
            log::warn!("ocr: index file is not valid JSON ({error}); treating it as empty");
            OcrIndex::default()
        }
    }
}

/// Loads the OCR index; a missing/unreadable/corrupt file yields an empty index (never an error).
pub fn load_index() -> OcrIndex {
    load_index_at(&match index_path() {
        Ok(path) => path,
        Err(error) => {
            log::warn!("ocr: cannot locate the index file: {error:#}");
            return OcrIndex::default();
        }
    })
}

/// [`load_index`] for an explicit path (unit-tested without `%LOCALAPPDATA%`).
fn load_index_at(path: &Path) -> OcrIndex {
    match fs::read(path) {
        Ok(bytes) => parse_index(&bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => OcrIndex::default(),
        Err(error) => {
            log::warn!("ocr: cannot read the index file {}: {error}", path.display());
            OcrIndex::default()
        }
    }
}

/// Persists the index atomically (`ocr-index.json.tmp` + rename over the previous file).
pub fn save_index(index: &OcrIndex) -> anyhow::Result<()> {
    let path = index_path()?;
    save_index_at(&path, index)?;
    Ok(())
}

/// [`save_index`] for an explicit path (atomic-write unit test).
fn save_index_at(path: &Path, index: &OcrIndex) -> anyhow::Result<()> {
    let bytes = serde_json::to_vec_pretty(index).context("cannot serialize the OCR index")?;
    let temp = path.with_extension("json.tmp");
    fs::write(&temp, bytes)
        .with_context(|| format!("cannot write the temporary OCR index {}", temp.display()))?;
    fs::rename(&temp, path).with_context(|| {
        format!(
            "cannot rename {} to {}",
            temp.display(),
            path.display()
        )
    })?;
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Search
// ---------------------------------------------------------------------------------------------

/// Case-insensitive first-match excerpt of `text` for the already-lowercased `needle`.
///
/// The window is [`EXCERPT_RADIUS`] characters wide on each side and is clamped to the text; the
/// query itself is always included when it matches. Pure (unit-tested at the start/middle/end).
fn excerpt_for(text: &str, needle: &str) -> Option<String> {
    let haystack = text.to_lowercase();
    let position = haystack.find(needle)?;
    let prefix_chars = haystack[..position].chars().count();
    let needle_chars = needle.chars().count();
    let total_chars = text.chars().count();
    let start = prefix_chars.saturating_sub(EXCERPT_RADIUS);
    let end = (prefix_chars + needle_chars + EXCERPT_RADIUS).min(total_chars);
    Some(text.chars().skip(start).take(end - start).collect())
}

/// Searches the persisted OCR index (case-insensitive substring); an empty query returns no hits.
pub fn search(query: &str) -> Vec<OcrHit> {
    load_index().search(query)
}

// ---------------------------------------------------------------------------------------------
// Index build
// ---------------------------------------------------------------------------------------------

/// Indexes the still-image items on the line: serial, low-priority, one atomic write at the end.
///
/// `force` re-runs OCR for unchanged hashes; `limit` caps the number of *attempts* (`0` = no cap).
/// Per-item failures are logged and counted, never fatal. The caller (the `build_ocr_index`
/// command) owns the single-flight guard and the worker thread.
pub fn build(app: &AppHandle, force: bool, limit: usize) -> Result<OcrBuildReport, String> {
    let started = Instant::now();
    let availability = availability();
    if !availability.available {
        return Ok(OcrBuildReport {
            attempted: 0,
            indexed: 0,
            skipped: 0,
            failed: 0,
            elapsed_ms: started.elapsed().as_millis() as u64,
            available: false,
        });
    }
    let Some(state) = app.try_state::<AppState>() else {
        return Err("OCR index: the app state is not managed".to_string());
    };
    lower_thread_priority();

    let mut index = load_index();
    if index.language.is_none() {
        index.language = availability.language.clone();
    }
    let mut report = OcrBuildReport {
        attempted: 0,
        indexed: 0,
        skipped: 0,
        failed: 0,
        elapsed_ms: 0,
        available: true,
    };

    let items: Vec<_> = state
        .snapshot()
        .items
        .into_iter()
        .filter(|item| item.kind != ItemKind::Recording && is_supported_image(&item.path))
        .collect();
    log::info!(
        "ocr: build started ({} candidate image(s), force={force}, limit={limit})",
        items.len()
    );
    for item in items {
        if limit > 0 && report.attempted >= limit {
            log::info!("ocr: attempt limit {limit} reached; stopping the build");
            break;
        }
        if !index.entry_needs_index(&item.id, item.hash, force) {
            report.skipped += 1;
            continue;
        }
        report.attempted += 1;
        match recognize_file(&item.path) {
            Ok(recognized) => {
                index.upsert(&item.id, item.hash, &recognized.text, crate::item::now_ms());
                report.indexed += 1;
                log::info!(
                    "ocr: indexed {} ({} line(s), {} char(s))",
                    item.path.display(),
                    recognized.lines.len(),
                    recognized.text.chars().count()
                );
            }
            Err(error) => {
                report.failed += 1;
                log::warn!("ocr: cannot recognize {}: {error}", item.path.display());
            }
        }
    }

    if report.indexed > 0 {
        save_index(&index).map_err(|error| {
            log::warn!("ocr: cannot persist the index: {error:#}");
            format!("cannot persist the OCR index: {error:#}")
        })?;
    }
    report.elapsed_ms = started.elapsed().as_millis() as u64;
    log::info!(
        "ocr: build finished (attempted {}, indexed {}, skipped {}, failed {}, {} ms)",
        report.attempted,
        report.indexed,
        report.skipped,
        report.failed,
        report.elapsed_ms
    );
    Ok(report)
}

// ---------------------------------------------------------------------------------------------
// CLI probe
// ---------------------------------------------------------------------------------------------

/// Runs `--ocr-probe <dir>`: one JSON line per image, a summary line, exit code = failures (0/1).
///
/// This never touches Tauri/AppState: it is the offline evaluation probe used before deciding
/// whether OCR ships. Unreadable directories and empty directories are reported as a summary with
/// zero files (exit code 0) so scripts can distinguish "no images" from a crash.
pub fn run_probe(dir: &Path) -> i32 {
    let files = match probe_images(dir) {
        Ok(files) => files,
        Err(error) => {
            eprintln!("hangline: cannot read {}: {error}", dir.display());
            println!(
                "{}",
                serde_json::json!({ "files": 0, "ok": 0, "failed": 1 })
            );
            return 1;
        }
    };
    log::info!("ocr-probe: {} image(s) in {}", files.len(), dir.display());
    let mut ok = 0usize;
    let mut failed = 0usize;
    for path in &files {
        let started = Instant::now();
        let elapsed_ms = |start: Instant| start.elapsed().as_millis() as u64;
        match recognize_file(path) {
            Ok(recognized) => {
                ok += 1;
                println!(
                    "{}",
                    serde_json::json!({
                        "file": path.to_string_lossy(),
                        "ok": true,
                        "text": recognized.text,
                        "lines": recognized.lines.len(),
                        "ms": elapsed_ms(started),
                    })
                );
            }
            Err(error) => {
                failed += 1;
                println!(
                    "{}",
                    serde_json::json!({
                        "file": path.to_string_lossy(),
                        "ok": false,
                        "text": "",
                        "lines": 0,
                        "ms": elapsed_ms(started),
                        "error": error,
                    })
                );
            }
        }
    }
    println!(
        "{}",
        serde_json::json!({ "files": files.len(), "ok": ok, "failed": failed })
    );
    i32::from(failed > 0)
}

/// Collects the image files of `dir` (non-recursive, supported extensions), sorted by path.
fn probe_images(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() && is_supported_image(&path) {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Scratch directory under `target/` (git-ignored); never touches `%LOCALAPPDATA%`.
    fn test_dir(tag: &str) -> PathBuf {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("ocr-tests")
            .join(format!("{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create the OCR test dir");
        dir
    }

    /// The OCR bitmap is capped at `MAX_OCR_EDGE` without ever upscaling; the other edge keeps the
    /// aspect ratio and can never become zero.
    #[test]
    fn target_dimensions_caps_the_long_edge_without_upscaling() {
        assert_eq!(target_dimensions(1000, 500, MAX_OCR_EDGE), (1000, 500));
        assert_eq!(target_dimensions(4000, 3000, MAX_OCR_EDGE), (2560, 1920));
        assert_eq!(target_dimensions(3000, 4000, MAX_OCR_EDGE), (1920, 2560));
        assert_eq!(target_dimensions(5000, 5000, MAX_OCR_EDGE), (2560, 2560));
        // A very thin image keeps at least one pixel on the short edge.
        assert_eq!(target_dimensions(5000, 1, MAX_OCR_EDGE), (2560, 1));
        // Degenerate input cannot panic (defensive: decoded images have non-zero dimensions).
        assert_eq!(target_dimensions(0, 0, MAX_OCR_EDGE), (1, 1));
    }

    /// Only still images are indexed; recordings/other files are skipped before any OCR work.
    #[test]
    fn supported_images_are_png_and_jpeg_only() {
        for name in ["shot.png", "shot.PNG", "photo.jpg", "photo.JPEG"] {
            assert!(is_supported_image(Path::new(name)), "{name} must be indexed");
        }
        for name in ["clip.mp4", "notes.txt", "no-extension", "shot.png.tmp"] {
            assert!(!is_supported_image(Path::new(name)), "{name} must be skipped");
        }
    }

    /// Hash-keyed incrementality: unchanged rows are skipped, a rewritten file (new hash) and an
    /// explicit `force` are re-indexed.
    #[test]
    fn entries_are_rebuilt_only_when_the_hash_changes_or_force_is_set() {
        let mut index = OcrIndex::default();
        assert!(index.entry_needs_index("a", 1, false), "a new item needs indexing");
        index.upsert("a", 1, "hello", 7);
        assert!(!index.entry_needs_index("a", 1, false));
        assert!(index.entry_needs_index("a", 2, false), "a new hash means a rewritten file");
        assert!(index.entry_needs_index("a", 1, true), "force rebuilds unchanged rows");
        assert!(index.entry_needs_index("b", 1, false));
    }

    /// One row per item id: an upsert replaces the old text/hash/timestamp.
    #[test]
    fn upsert_replaces_the_existing_entry() {
        let mut index = OcrIndex::default();
        index.upsert("a", 1, "old", 1);
        index.upsert("a", 2, "new", 2);
        assert_eq!(index.entries.len(), 1);
        assert_eq!(
            index.entries.get("a"),
            Some(&OcrIndexEntry {
                hash: 2,
                text: "new".to_string(),
                indexed_at: 2,
            })
        );
    }

    /// The index file is the documented camelCase shape; serde defaults keep a partial file usable.
    #[test]
    fn index_serializes_as_camel_case_and_tolerates_missing_fields() {
        let mut index = OcrIndex::default();
        index.language = Some("zh-Hans".to_string());
        index.upsert("item-1", 42, "截图文字", 1_700_000_000_000);
        let value = serde_json::to_value(&index).expect("serialize the OCR index");
        assert_eq!(
            value,
            serde_json::json!({
                "version": INDEX_VERSION,
                "language": "zh-Hans",
                "entries": {
                    "item-1": {
                        "hash": 42,
                        "text": "截图文字",
                        "indexedAt": 1_700_000_000_000i64,
                    }
                }
            })
        );
        let parsed: OcrIndex = serde_json::from_slice(&serde_json::to_vec(&index).unwrap())
            .expect("deserialize the OCR index");
        assert_eq!(parsed, index);

        // A hand-edited/partial file falls back to the defaults instead of failing.
        let partial: OcrIndex = serde_json::from_slice(b"{}").expect("empty object uses defaults");
        assert_eq!(partial, OcrIndex::default());
    }

    /// A corrupt file or an unknown schema version is an empty index with a warning, never a panic.
    #[test]
    fn corrupt_and_future_version_indexes_are_treated_as_empty() {
        assert_eq!(parse_index(b"{ this is not json"), OcrIndex::default());
        assert_eq!(parse_index(b""), OcrIndex::default());
        let future = serde_json::json!({
            "version": INDEX_VERSION + 1,
            "language": "en",
            "entries": { "a": { "hash": 1, "text": "x", "indexedAt": 2 } },
        });
        assert_eq!(
            parse_index(future.to_string().as_bytes()),
            OcrIndex::default()
        );
    }

    /// Search is a case-insensitive substring match (ASCII case folding, Chinese exact characters)
    /// and an empty/blank query is a no-op.
    #[test]
    fn search_is_case_insensitive_and_matches_chinese() {
        let mut index = OcrIndex::default();
        index.upsert("a", 1, "Hello World, the QUICK fox", 0);
        index.upsert("b", 1, "微信聊天记录：你好，世界", 0);
        index.upsert("c", 1, "unrelated", 0);

        assert_eq!(
            index.search("hello"),
            vec![OcrHit {
                id: "a".to_string(),
                excerpt: "Hello World, the QUICK fox".to_string(),
            }]
        );
        assert_eq!(
            index.search("quick fox"),
            vec![OcrHit {
                id: "a".to_string(),
                excerpt: "Hello World, the QUICK fox".to_string(),
            }]
        );
        assert_eq!(
            index.search("世界"),
            vec![OcrHit {
                id: "b".to_string(),
                excerpt: "微信聊天记录：你好，世界".to_string(),
            }]
        );
        assert!(index.search("missing").is_empty());
        assert!(index.search("").is_empty());
        assert!(index.search("   ").is_empty());
        assert!(index.search("WORLD").len() == 1, "case-insensitive");
    }

    /// The excerpt keeps at most 40 characters on each side, clamps at the text edges and always
    /// contains the match.
    #[test]
    fn excerpt_window_is_40_chars_each_side_and_clamped() {
        let long = format!("{}MATCH{}", "a".repeat(100), "b".repeat(100));
        let excerpt = excerpt_for(&long, "match").expect("a match");
        assert_eq!(excerpt.chars().count(), EXCERPT_RADIUS * 2 + 5);
        assert!(excerpt.contains("MATCH"));
        assert_eq!(
            excerpt.chars().take(EXCERPT_RADIUS).all(|character| character == 'a'),
            true
        );

        // Match at the very start / very end: no negative window, no panic.
        let start = excerpt_for("MATCH then text", "match").expect("start match");
        assert_eq!(start, "MATCH then text");
        let end = excerpt_for("text then MATCH", "match").expect("end match");
        assert_eq!(end, "text then MATCH");
        assert!(excerpt_for("no hit here", "missing").is_none());
    }

    /// `save_index_at` writes atomically and `load_index_at` reads the exact same document back;
    /// a corrupt file on disk loads as empty instead of failing.
    #[test]
    fn index_save_load_round_trips_and_survives_corruption() {
        let dir = test_dir("save-load");
        let path = dir.join(INDEX_FILE);
        let mut index = OcrIndex::default();
        index.language = Some("zh-Hans".to_string());
        index.upsert("item-1", 42, "hello 世界", 123);
        save_index_at(&path, &index).expect("save the index");
        assert!(path.is_file());
        assert!(!path.with_extension("json.tmp").exists(), "no .tmp left behind");
        assert_eq!(load_index_at(&path), index);

        // Loading a corrupt file is tolerated (treated as empty); the next build rewrites it.
        fs::write(&path, b"not json").expect("corrupt the index");
        assert_eq!(load_index_at(&path), OcrIndex::default());
        assert_eq!(load_index_at(&dir.join("missing.json")), OcrIndex::default());
        let _ = fs::remove_dir_all(dir);
    }

    /// The probe collects only supported image files, sorted, and ignores directories/recordings.
    #[test]
    fn probe_collects_sorted_supported_images() {
        let dir = test_dir("probe-collect");
        for name in ["b.png", "a.JPG", "clip.mp4", "notes.txt"] {
            fs::write(dir.join(name), b"not decoded by the collector").expect("write fixture");
        }
        fs::create_dir(dir.join("sub")).expect("create subdir");
        let files = probe_images(&dir).expect("collect");
        assert_eq!(
            files,
            vec![dir.join("a.JPG"), dir.join("b.png")],
            "only supported still images, sorted"
        );
        let _ = fs::remove_dir_all(dir);
    }

    /// The build report is the frontend contract shape (camelCase, no hidden fields).
    #[test]
    fn build_report_serializes_as_camel_case() {
        let report = OcrBuildReport {
            attempted: 3,
            indexed: 2,
            skipped: 1,
            failed: 1,
            elapsed_ms: 900,
            available: true,
        };
        assert_eq!(
            serde_json::to_value(&report).expect("serialize the build report"),
            serde_json::json!({
                "attempted": 3,
                "indexed": 2,
                "skipped": 1,
                "failed": 1,
                "elapsedMs": 900,
                "available": true,
            })
        );
    }

    /// Real-Windows evidence (manual): set `HANG_TEST_OCR_IMAGE` to a PNG/JPG with text and run
    /// `cargo test --lib ocr::tests::real_ocr -- --ignored --nocapture`.
    #[test]
    #[ignore = "real Windows.Media.Ocr: set HANG_TEST_OCR_IMAGE=<image with text>"]
    fn real_ocr_reads_a_text_image() {
        let Some(path) = std::env::var_os("HANG_TEST_OCR_IMAGE") else {
            eprintln!("HANG_TEST_OCR_IMAGE is not set; skipping the real OCR evidence");
            return;
        };
        let recognized = recognize_file(Path::new(&path)).expect("real OCR must succeed");
        eprintln!(
            "ocr: {} line(s), {} char(s): {:?}",
            recognized.lines.len(),
            recognized.text.chars().count(),
            recognized.text
        );
        assert!(
            !recognized.text.trim().is_empty(),
            "a text image must yield text"
        );
    }

    /// Real-Windows evidence for the availability report (manual probe, never in CI).
    #[test]
    #[ignore = "real Windows.Media.Ocr: needs an installed OCR language pack"]
    fn real_availability_reports_the_engine() {
        let availability = availability();
        eprintln!("ocr availability: {availability:?}");
        assert!(
            availability.available,
            "expected a Windows OCR language pack: {availability:?}"
        );
        assert!(availability.language.is_some());
    }
}
