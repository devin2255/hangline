//! Watches folders with `notify` and turns new image/recording files into items.
//!
//! Design spec 3.2 sources B (system screenshots), D (Xbox Game Bar recordings), plus our own
//! capture folder and up to three user-configured folders. Each watched folder gets its own
//! `notify` watcher and one debounce worker: a capture often produces several OS events (create,
//! size, data chunks), which are coalesced into one callback ~200 ms after the writes stop.
//!
//! Nothing here runs on the UI thread: `notify` delivers events on its own thread and each watcher
//! owns one worker thread. Dropping a [`DirWatcher`] closes the event channel and joins the
//! worker, so a stopped watcher can never touch the app state afterwards.
//!
//! Dedupe happens in three layers (design spec 3.2):
//! 1. the candidate filter (pure, below) drops temp/hidden/non-image files;
//! 2. the debounce worker drops repeated `(path, mtime, size)` notifications;
//! 3. [`crate::dedupe`] drops the same *content* arriving twice (system folder + clipboard +
//!    own capture), and [`crate::app_state::AppState::contains_path`] drops a path already hung.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use tauri::{AppHandle, Emitter, EventTarget, Manager};

use crate::app_state::{self, AppState};
use crate::dedupe::{self, Deduper};
use crate::item::{Item, ItemKind};

/// How long a folder must stay quiet before its queued events are processed.
pub const DEBOUNCE: Duration = Duration::from_millis(200);

/// Upper bound on one debounce batch, so a continuously written folder cannot starve delivery.
const MAX_BATCH_WAIT: Duration = Duration::from_secs(2);

/// Cap of the per-folder `(path, mtime, size)` cache (content dedupe is the real duplicate guard).
const DELIVERED_CACHE: usize = 512;

/// Image extensions accepted in every watched folder (design spec 3.2 "只认图片扩展名").
const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "heic", "tif", "tiff", "gif", "webp"];

/// Video extensions accepted only in recording folders (Xbox Game Bar).
const RECORDING_EXTENSIONS: &[&str] = &["mov", "mp4"];

/// `reason` payload of the `reveal-request` event emitted when a new item hangs on the line.
///
/// Design spec 4.4 lists `reveal`/`tuck` as `{ reason }`. The watcher only announces that the
/// line has a new item; Task 11's reveal controller owns the state machine and emits the
/// frontend-facing `reveal { reason, peek }`. The request uses its own Rust-only event name so
/// it can never be mistaken for the frontend event (JS listeners register as `Any` and would
/// otherwise observe the raw request too).
pub(crate) const REVEAL_REASON_NEW_ITEM: &str = "newItem";

/// What a watched folder accepts and which [`ItemKind`] its image files map to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Opts {
    /// Provenance for accepted image files (recording folders map videos to `Recording`, see
    /// [`Opts::item_kind`]).
    pub kind: ItemKind,
    /// Whether `mov`/`mp4` files are accepted in this folder.
    pub accept_recordings: bool,
}

impl Opts {
    /// `%USERPROFILE%\Pictures\Screenshots`: images only, `SystemScreenshot`.
    pub fn system_screenshots() -> Self {
        Self {
            kind: ItemKind::SystemScreenshot,
            accept_recordings: false,
        }
    }

    /// `%USERPROFILE%\Videos\Captures`: videos as `Recording`, images as `SystemScreenshot`.
    ///
    /// Xbox Game Bar writes both recordings and screenshots into this folder, so an image here is
    /// still a system screenshot.
    pub fn recordings() -> Self {
        Self {
            kind: ItemKind::SystemScreenshot,
            accept_recordings: true,
        }
    }

    /// `%LOCALAPPDATA%\Hangline\Screenshots`: our own captures, images only, `Capture`.
    pub fn own_captures() -> Self {
        Self {
            kind: ItemKind::Capture,
            accept_recordings: false,
        }
    }

    /// A user-configured folder: any image, never recordings, `WatchedFolder`.
    pub fn watch_any_image() -> Self {
        Self {
            kind: ItemKind::WatchedFolder,
            accept_recordings: false,
        }
    }

    /// The [`ItemKind`] for one accepted file.
    ///
    /// Game Bar also saves screenshots into `Videos\Captures`; those are images, not recordings.
    pub fn item_kind(&self, path: &Path) -> ItemKind {
        if self.accept_recordings && is_recording(path) {
            ItemKind::Recording
        } else {
            self.kind
        }
    }
}

/// Returns whether `path` is a file the folder described by `opts` should hang on the line.
///
/// Pure (no filesystem access): extension whitelist, dot-files and `.tmp`/`.part` files are
/// rejected, and recordings are only accepted in recording folders. `is_file` is checked later,
/// when the debounced path is read.
pub fn is_candidate(path: &Path, opts: Opts) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    if name.starts_with('.') {
        return false;
    }
    let lower_name = name.to_ascii_lowercase();
    if lower_name.ends_with(".tmp") || lower_name.ends_with(".part") {
        return false;
    }
    let Some(extension) = extension(path) else {
        return false;
    };
    IMAGE_EXTENSIONS.contains(&extension.as_str())
        || (opts.accept_recordings && RECORDING_EXTENSIONS.contains(&extension.as_str()))
}

/// Lowercase extension of `path`, when it has one.
fn extension(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
}

/// Whether `path` has one of the recording extensions.
fn is_recording(path: &Path) -> bool {
    extension(path).is_some_and(|extension| RECORDING_EXTENSIONS.contains(&extension.as_str()))
}

/// One running folder watcher; dropping it stops event delivery and joins the debounce worker.
///
/// The watcher is stored behind an `Option` so [`Drop`] can stop event delivery *before* joining
/// the worker (the worker exits when the notify callback, and therefore the channel sender, is
/// dropped).
pub struct DirWatcher {
    folder: PathBuf,
    watcher: Option<RecommendedWatcher>,
    worker: Option<JoinHandle<()>>,
}

impl Drop for DirWatcher {
    fn drop(&mut self) {
        // Dropping the notify watcher drops its callback and with it the channel sender; the
        // worker then sees the disconnect and exits. Joining guarantees no callback runs after the
        // watcher is gone (bounded by MAX_BATCH_WAIT + the callback's own I/O).
        self.watcher.take();
        if let Some(worker) = self.worker.take() {
            if worker.join().is_err() {
                log::warn!(
                    "watchers: debounce worker for {} panicked",
                    self.folder.display()
                );
            }
        }
        log::debug!("watchers: stopped watching {}", self.folder.display());
    }
}

/// Starts watching `folder` with the production item pipeline (AppState + `state` event).
pub fn start(app: &AppHandle, folder: &Path, opts: Opts) -> anyhow::Result<DirWatcher> {
    let app = app.clone();
    start_with(folder, opts, move |path| ingest(&app, path, opts))
}

/// Starts watching `folder` and calls `on_new` for every debounced candidate file.
///
/// This is the seam the integration tests use; the production [`start`] wraps it with the
/// AppState/dedupe pipeline. A missing folder is an error (callers log and skip it); this function
/// never creates folders.
pub fn start_with<F>(folder: &Path, opts: Opts, on_new: F) -> anyhow::Result<DirWatcher>
where
    F: Fn(&Path) + Send + 'static,
{
    if !folder.is_dir() {
        anyhow::bail!("cannot watch {}: not an existing directory", folder.display());
    }

    let (sender, receiver) = mpsc::channel::<PathBuf>();
    let callback_folder = folder.to_path_buf();
    let mut watcher = notify::recommended_watcher(
        move |result: notify::Result<notify::Event>| match result {
            Ok(event) => deliver_event(&sender, &event, opts),
            Err(error) => log::debug!(
                "watchers: notify error in {}: {error}",
                callback_folder.display()
            ),
        },
    )
    .map_err(|error| {
        anyhow::anyhow!(
            "failed to create a folder watcher for {}: {error}",
            folder.display()
        )
    })?;

    watcher
        .watch(folder, RecursiveMode::Recursive)
        .map_err(|error| {
            anyhow::anyhow!(
                "failed to watch {} (missing or denied?): {error}",
                folder.display()
            )
        })?;

    let worker = thread::Builder::new()
        .name("hangline-folder-watcher".to_string())
        .spawn(move || debounce_loop(receiver, opts, on_new))
        .map_err(|error| {
            anyhow::anyhow!(
                "failed to spawn the debounce worker for {}: {error}",
                folder.display()
            )
        })?;

    log::info!("watchers: watching {} ({opts:?})", folder.display());
    Ok(DirWatcher {
        folder: folder.to_path_buf(),
        watcher: Some(watcher),
        worker: Some(worker),
    })
}

/// Queues the candidate paths of one notify event.
fn deliver_event(sender: &Sender<PathBuf>, event: &notify::Event, opts: Opts) {
    for path in &event.paths {
        if is_candidate(path, opts) && sender.send(path.clone()).is_err() {
            // The debounce worker is gone (watcher dropping); stop forwarding quietly.
            return;
        }
    }
}

/// Collects events until the folder has been quiet for [`DEBOUNCE`], then delivers the batch.
///
/// The deadline slides with every new event (one capture = one batch), capped at
/// [`MAX_BATCH_WAIT`]. A path repeated inside one batch is delivered once; a path repeated across
/// batches with an unchanged `(mtime, size)` is dropped as well (spec 3.2 rule 3).
fn debounce_loop<F>(receiver: Receiver<PathBuf>, opts: Opts, on_new: F)
where
    F: Fn(&Path),
{
    let mut delivered: HashMap<PathBuf, (SystemTime, u64)> = HashMap::new();
    loop {
        let Ok(first) = receiver.recv() else {
            // All senders (the notify callback) are gone: the watcher was dropped.
            break;
        };

        let mut batch = vec![first];
        let started = Instant::now();
        let mut deadline = started + DEBOUNCE;
        loop {
            let wait_until = deadline.min(started + MAX_BATCH_WAIT);
            let Some(remaining) = wait_until.checked_duration_since(Instant::now()) else {
                break;
            };
            match receiver.recv_timeout(remaining) {
                Ok(path) => {
                    if !batch.contains(&path) {
                        batch.push(path);
                    }
                    deadline = Instant::now() + DEBOUNCE;
                }
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }

        for path in batch {
            deliver_one(&path, &mut delivered, &on_new);
        }
    }
    log::debug!("watchers: debounce worker ({:?}) stopped", opts.kind);
}

/// Delivers one debounced path when it is a readable, non-empty file not already delivered.
fn deliver_one<F>(path: &Path, delivered: &mut HashMap<PathBuf, (SystemTime, u64)>, on_new: &F)
where
    F: Fn(&Path),
{
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) => {
            // Removed between the event and the debounce flush (or a permission problem): the
            // folder watcher must never panic or spam, so this is a debug-level miss.
            log::debug!("watchers: cannot stat {}: {error}", path.display());
            return;
        }
    };
    if !metadata.is_file() {
        return;
    }
    if metadata.len() == 0 {
        log::debug!(
            "watchers: ignoring empty file {} (still being written?)",
            path.display()
        );
        return;
    }

    let signature = (
        metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
        metadata.len(),
    );
    if delivered.get(path) == Some(&signature) {
        log::debug!("watchers: unchanged repeat for {}", path.display());
        return;
    }

    on_new(path);
    if delivered.len() >= DELIVERED_CACHE {
        // Bounded memory for long sessions; content dedupe still catches a repeated old file.
        delivered.clear();
    }
    delivered.insert(path.to_path_buf(), signature);
}

/// Production callback: hash, dedupe, read dimensions, append to `AppState`, emit `state`.
///
/// The item is persisted by [`AppState::add_item`] before the snapshot is broadcast, so the
/// frontend never sees an item that a restart would forget. Failures are logged; a watcher never
/// panics the app.
fn ingest(app: &AppHandle, path: &Path, opts: Opts) {
    if !is_candidate(path, opts) {
        log::debug!(
            "watchers: {} stopped looking like a candidate",
            path.display()
        );
        return;
    }

    let state = app.state::<AppState>();
    if state.contains_path(path) {
        log::info!(
            "watchers: ignored {} (already hung on the line)",
            path.display()
        );
        return;
    }

    let hash = match dedupe::hash_file(path) {
        Ok(hash) => hash,
        Err(error) => {
            log::warn!("watchers: cannot hash {}: {error}", path.display());
            return;
        }
    };
    let deduper = app.state::<Deduper>();
    if !deduper.check_or_insert(hash) {
        log::info!(
            "watchers: dedupe hit for {} (hash {hash:016x})",
            path.display()
        );
        return;
    }

    let (width, height) = dimensions(path);
    let kind = opts.item_kind(path);
    let mut item = Item::new(path.to_path_buf(), kind, hash, width, height);
    // P2-7: generate the preview before the first snapshot; a failure keeps the original path.
    crate::thumbnails::attach(&mut item);
    match state.add_item(item) {
        Ok(_) => {
            log::info!(
                "watchers: hung {} ({kind:?}) {width}x{height} hash {hash:016x}",
                path.display()
            );
            // The single serialized snapshot+broadcast choke point. The snapshot it returns is the
            // freshest state, so the reveal decision below cannot be made from a state that a
            // concurrent `set_setting` already superseded.
            let Some(snapshot) = app_state::emit_state(app) else {
                return;
            };
            // Design spec 3.4: a new screenshot peeks the line when `revealOnNew` is on. This is a
            // Rust-only request to the reveal controller (Task 11); the controller decides whether
            // the line is currently dismissed and emits the frontend `reveal { reason, peek }`.
            if snapshot.settings.reveal_on_new {
                if let Err(error) = app.emit_to(
                    EventTarget::App,
                    "reveal-request",
                    serde_json::json!({ "reason": REVEAL_REASON_NEW_ITEM }),
                ) {
                    log::warn!("watchers: failed to emit the reveal request: {error}");
                }
            }
        }
        Err(error) => {
            log::error!(
                "watchers: failed to persist the new item for {}: {error:#}",
                path.display()
            );
            // `add_item` mutated memory before the persist failure, so the hash must not stay
            // claimed in the dedupe LRU either: a retry of the same content would otherwise be
            // swallowed as a duplicate (review minor 8).
            deduper.forget(hash);
            // Republish so the UI matches memory; the revision advanced with the mutation.
            app_state::emit_state(app);
        }
    }
}

/// Pixel dimensions of an image, `0x0` for recordings (spec 4.5).
///
/// Only the image header is read. A format `image` cannot decode (e.g. HEIC without a decoder)
/// still hangs on the line with `0x0`, because the file itself is a valid screenshot candidate.
fn dimensions(path: &Path) -> (u32, u32) {
    if is_recording(path) {
        return (0, 0);
    }
    match image::image_dimensions(path) {
        Ok((width, height)) => (width, height),
        Err(error) => {
            log::warn!(
                "watchers: cannot read image dimensions of {}: {error}",
                path.display()
            );
            (0, 0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    use base64::prelude::*;

    /// 1x1 transparent PNG (valid image fixture, 68 bytes).
    const PNG_1X1_BASE64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

    /// Scratch directory under `target/` (git-ignored).
    fn test_dir(tag: &str) -> PathBuf {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("watcher-tests")
            .join(format!("{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create the watcher test dir");
        dir
    }

    /// Decodes the 1x1 PNG fixture.
    fn png_1x1() -> Vec<u8> {
        BASE64_STANDARD
            .decode(PNG_1X1_BASE64)
            .expect("the 1x1 PNG fixture must be valid base64")
    }

    #[test]
    fn candidate_rules_follow_extension_and_temp_rules() {
        use ItemKind::SystemScreenshot;

        let cases = [
            ("a.PNG", Opts::system_screenshots(), true),
            ("a.jpg", Opts::system_screenshots(), true),
            ("a.JPEG", Opts::system_screenshots(), true),
            ("a.heic", Opts::system_screenshots(), true),
            ("a.tif", Opts::system_screenshots(), true),
            ("a.tiff", Opts::system_screenshots(), true),
            ("a.gif", Opts::system_screenshots(), true),
            ("a.webp", Opts::system_screenshots(), true),
            ("a.txt", Opts::system_screenshots(), false),
            ("a.mp4", Opts::system_screenshots(), false),
            ("a.mp4", Opts::recordings(), true),
            ("a.MOV", Opts::recordings(), true),
            ("a.png", Opts::recordings(), true),
            ("a.png", Opts::own_captures(), true),
            ("a.mp4", Opts::own_captures(), false),
            ("a.png", Opts::watch_any_image(), true),
            ("a.mp4", Opts::watch_any_image(), false),
            (".hidden.png", Opts::watch_any_image(), false),
            ("shot.png.tmp", Opts::watch_any_image(), false),
            ("shot.part", Opts::watch_any_image(), false),
            ("no-extension", Opts::watch_any_image(), false),
        ];
        for (path, opts, expected) in cases {
            assert_eq!(
                is_candidate(Path::new(path), opts),
                expected,
                "is_candidate({path:?}, {opts:?})"
            );
        }
        // The pure filter must not depend on the process working directory or the file existing.
        assert_eq!(
            Opts::system_screenshots().kind,
            SystemScreenshot,
            "screenshots keep their provenance"
        );
    }

    #[test]
    fn item_kind_follows_the_watched_folder() {
        assert_eq!(
            Opts::system_screenshots().item_kind(Path::new("shot.png")),
            ItemKind::SystemScreenshot
        );
        assert_eq!(
            Opts::recordings().item_kind(Path::new("clip.mp4")),
            ItemKind::Recording,
            "a recording folder maps videos to Recording"
        );
        assert_eq!(
            Opts::recordings().item_kind(Path::new("shot.png")),
            ItemKind::SystemScreenshot,
            "Game Bar screenshots also land in Videos\\Captures"
        );
        assert_eq!(
            Opts::own_captures().item_kind(Path::new("shot.png")),
            ItemKind::Capture
        );
        assert_eq!(
            Opts::watch_any_image().item_kind(Path::new("photo.jpg")),
            ItemKind::WatchedFolder
        );
    }

    #[test]
    fn temp_dir_watcher_reports_a_new_png_within_two_seconds() {
        let dir = test_dir("notify-png");
        let (tx, rx) = mpsc::channel();
        let watcher = start_with(&dir, Opts::system_screenshots(), move |path| {
            let _ = tx.send(path.to_path_buf());
        })
        .expect("start the temp-dir watcher");
        // ReadDirectoryChangesW registration is synchronous, but give the OS a beat before writing.
        thread::sleep(Duration::from_millis(100));

        let path = dir.join("shot.png");
        fs::write(&path, png_1x1()).expect("write the 1x1 PNG");

        assert_eq!(
            rx.recv_timeout(Duration::from_secs(2))
                .expect("the new PNG must be reported within 2 s"),
            path
        );
        drop(watcher);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn temp_dir_watcher_ignores_non_candidates_but_stays_alive() {
        let dir = test_dir("notify-filter");
        let (tx, rx) = mpsc::channel();
        let watcher = start_with(&dir, Opts::system_screenshots(), move |path| {
            let _ = tx.send(path.to_path_buf());
        })
        .expect("start the temp-dir watcher");
        thread::sleep(Duration::from_millis(100));

        fs::write(dir.join("notes.txt"), b"not an image").expect("write the text file");
        fs::write(dir.join("shot.png.tmp"), png_1x1()).expect("write the temp image");
        assert!(
            rx.recv_timeout(Duration::from_millis(600)).is_err(),
            ".txt and .tmp files must not reach the callback"
        );

        // The watcher must still deliver real candidates after ignoring the noise.
        let path = dir.join("real.png");
        fs::write(&path, png_1x1()).expect("write the real PNG");
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(2))
                .expect("the watcher must still be alive"),
            path
        );
        drop(watcher);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn temp_dir_watcher_debounces_a_burst_of_writes_into_one_callback() {
        let dir = test_dir("notify-debounce");
        let (tx, rx) = mpsc::channel();
        let watcher = start_with(&dir, Opts::system_screenshots(), move |path| {
            let _ = tx.send(path.to_path_buf());
        })
        .expect("start the temp-dir watcher");
        thread::sleep(Duration::from_millis(100));

        let path = dir.join("burst.png");
        for _ in 0..5 {
            fs::write(&path, png_1x1()).expect("write the burst PNG");
        }

        assert_eq!(
            rx.recv_timeout(Duration::from_secs(2))
                .expect("the burst must be reported"),
            path
        );
        assert!(
            rx.recv_timeout(Duration::from_millis(600)).is_err(),
            "one capture may emit several OS events but must produce one callback"
        );
        drop(watcher);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_folder_is_an_error_not_a_panic() {
        let dir = test_dir("missing").join("does-not-exist");
        let result = start_with(&dir, Opts::watch_any_image(), |_| {});
        assert!(
            result.is_err(),
            "a missing folder must be reported as an error"
        );
        assert!(!dir.exists(), "the watcher must never create user folders");
    }
}
