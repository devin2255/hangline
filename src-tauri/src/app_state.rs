//! The single source of truth for items and settings, and the `state` event payload.
//!
//! Design spec 4.4: Rust owns the data; the frontend mirrors it. Every command reads or mutates the
//! state behind one mutex, and every change is persisted before the snapshot is returned. The
//! `state` event carries the full [`PublicState`] (design spec 4.4 "全量状态").
//!
//! Every full-state broadcast goes through [`emit_state`], which serializes snapshot + emit under a
//! dedicated mutex and only emits revision-ordered snapshots. Without that choke point two folder
//! watcher threads plus the IPC thread could snapshot, interleave, and deliver an older
//! full state last; the frontend replaces its whole store, so a card/setting would silently
//! regress. [`AppState::broadcast`] is the testable snapshot+emit seam behind it.
//!
//! # Retention (M14)
//!
//! `keepOnLine` (design spec §3.6) caps the number of cards in the state:
//!
//! - [`KeepOnLine::Limit`] is a hard cap: after every add, and whenever the setting drops, the
//!   oldest cards (ascending `added_at`) leave the line until at most `n` remain. A fixed limit
//!   therefore never grows without bound.
//! - [`KeepOnLine::Fit`] is a *view* policy only: the panel renders the cards that fit, and the
//!   state itself is deliberately unbounded.
//!
//! Trimming **never deletes files**: retained-out cards only leave the state, while their source
//! files and other metadata stay on disk (design spec §4.8 narrowed by the P4 global constraint:
//! there is no "keep files" setting yet, so count retention is state-only). Their cached thumbnails
//! are left to the existing id-based orphan sweep (cache only), so the count cap adds no
//! file-deletion path. The age cap below is the one explicit exception: it recycles Hangline-owned
//! screenshots *after* the lock, through the same decision table as a manual take-down.
//!
//! # Age retention (design extension: `keepForDays`)
//!
//! `keepForDays` is a second, independent cap: [`AppState::trim_expired`] drops cards whose
//! `added_at` is older than the chosen 7/30/90-day window (or never, for the default
//! [`KeepForDays::Forever`]). The in-memory trim and one revision bump happen under the lock, and
//! the trimmed list is persisted under it like every other mutation; source-file recycling and
//! thumbnail removal happen after the lock (the caller — `retention::sweep` — also broadcasts the
//! snapshot and recycles the expired Hangline-owned screenshots). The two caps compose: a card
//! leaves the line when either one says so.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use anyhow::Context;
use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager};

use crate::geometry::PhysicalRect;
use crate::item::Item;
use crate::settings::{self, KeepForDays, KeepOnLine, Settings};
use crate::store;

/// How long a capture selection rect stays available for its fly-in (P2-5).
///
/// The panel asks for the rect right after the new-item `state` snapshot; 30 s covers a reveal
/// delayed by a tuck/peek cycle and expires stale entries if the panel never asked (e.g. the flight
/// was rejected because the overlay was busy).
const CAPTURE_ORIGIN_TTL: Duration = Duration::from_secs(30);

/// Maximum number of pending capture origins kept (P2-5 brief: bounded map of 8).
const CAPTURE_ORIGIN_MAX: usize = 8;

/// One capture rect waiting to be consumed by the fly-in overlay (keyed by the new item id).
#[derive(Debug)]
struct CaptureOrigin {
    /// Opaque item id of the capture that produced this rect.
    id: String,
    /// The selection rectangle on the virtual screen (physical px), i.e. the fly-in start rect.
    rect: PhysicalRect,
    /// When the capture finished; entries older than [`CAPTURE_ORIGIN_TTL`] are dropped.
    at: Instant,
}

/// Serialized application state sent to the frontend by `get_app_state` and the `state` event.
///
/// Design spec 4.4/4.5: `items` (oldest first), the full `settings` object and the app `version`.
/// The monitor list is deliberately not part of this payload: the panel asks for its window's
/// geometry per monitor (Task 10), so a monitor change does not have to republish the item list.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicState {
    /// Items hung on the line, ordered by `added_at` ascending.
    pub items: Vec<Item>,
    /// The 18 design-spec user settings plus the `keepForDays` retention extension.
    pub settings: Settings,
    /// Application version (Cargo/package version).
    pub version: String,
    /// Monotonic revision, incremented on every items/settings mutation.
    ///
    /// A full-state snapshot carries its revision, so the frontend can ignore one that arrives
    /// after a newer snapshot (and compare a `get_app_state` reply with events received while the
    /// fetch was in flight). See [`emit_state`].
    pub revision: u64,
    /// The language both the Rust tray and the panel must render (`"en"` or `"zh-Hans"`).
    ///
    /// Task 15 review fix: the `language` setting alone is ambiguous for `"system"` — the tray and
    /// the webview used to resolve it against different OS locale sources. Rust resolves it **once**
    /// (Windows display language first, then locale, then English) and broadcasts the result, so the
    /// frontend never re-derives it from `navigator.language`.
    pub resolved_language: String,
}

/// Application version reported in every snapshot.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// The mutable state behind the mutex.
struct Inner {
    /// `%LOCALAPPDATA%\Hangline` (where `settings.json` and `state.json` live).
    dir: PathBuf,
    items: Vec<Item>,
    settings: Settings,
    /// Advanced by every in-memory mutation, *before* the persist attempt.
    revision: u64,
    /// Capture selection rects waiting for the panel's fly-in request (P2-5, spec §4.6 C).
    capture_origins: VecDeque<CaptureOrigin>,
}

impl Inner {
    fn snapshot(&self) -> PublicState {
        PublicState {
            items: self.items.clone(),
            settings: self.settings.clone(),
            version: version().to_string(),
            revision: self.revision,
            resolved_language: crate::i18n::resolved_tag(&self.settings.language).to_string(),
        }
    }

    /// Records one items/settings mutation for the frontend revision check.
    ///
    /// Bumped before persisting: on a persist failure the in-memory state has already changed, and
    /// the caller republishes it (see the watcher `add_item` error path), so the revision must
    /// change too. A rejected mutation never reaches this helper.
    fn bump(&mut self) {
        self.revision += 1;
    }

    /// Drops capture origins older than [`CAPTURE_ORIGIN_TTL`].
    fn prune_capture_origins(&mut self, now: Instant) {
        self.capture_origins
            .retain(|origin| now.saturating_duration_since(origin.at) <= CAPTURE_ORIGIN_TTL);
    }

    /// Applies the age-retention policy to the in-memory list and logs when cards left.
    ///
    /// Runs under the state lock but performs no I/O (the caller persists), and it never touches
    /// files or thumbnails: recycling is the lifecycle half in `retention::sweep`.
    fn enforce_age_retention(&mut self, now_ms: i64) -> Vec<Item> {
        let expired = drop_expired(&mut self.items, self.settings.keep_for_days, now_ms);
        if !expired.is_empty() {
            log::info!(
                "age retention: dropped {} oldest card(s); keepForDays = {:?}; files handled by the sweep",
                expired.len(),
                self.settings.keep_for_days
            );
        }
        expired
    }

    /// Applies the retention cap to the in-memory list and logs one info line when cards left.
    ///
    /// Runs under the state lock but performs no I/O (the caller persists the trimmed list), and it
    /// never touches files or thumbnails: retention only edits the state (see the module docs).
    fn enforce_retention(&mut self) -> usize {
        let dropped = drop_oldest(&mut self.items, self.settings.keep_on_line);
        if dropped > 0 {
            log::info!(
                "retention: dropped {dropped} oldest card(s); keepOnLine = {:?}; files stay on disk",
                self.settings.keep_on_line
            );
        }
        dropped
    }
}

/// Number of cards `limit` allows to stay in the state.
///
/// [`KeepOnLine::Fit`] is a view policy and never caps the state; [`KeepOnLine::Limit`] keeps the
/// newest `n` cards (design spec §4.8 / M14). The panel capacity is a frontend concern and is not
/// part of the state cap. Pure so the retention arithmetic is unit-testable without a filesystem.
pub fn retained_len(limit: KeepOnLine, current: usize) -> usize {
    match limit {
        KeepOnLine::Fit => current,
        KeepOnLine::Limit(max) => current.min(usize::from(max)),
    }
}

/// Drops the oldest items until at most [`retained_len`] remain; returns the dropped count.
///
/// `items` must be sorted by `added_at` ascending (the invariant of every mutator and of
/// [`store::load_and_prune`]), so the rows removed are exactly the oldest ones. Only the in-memory
/// list changes: no file, thumbnail or watched-folder path is touched (see the module docs).
fn drop_oldest(items: &mut Vec<Item>, limit: KeepOnLine) -> usize {
    let retained = retained_len(limit, items.len());
    let dropped = items.len() - retained;
    if dropped > 0 {
        items.drain(..dropped);
    }
    dropped
}

/// Milliseconds in one day (age retention is day-granularity; no calendar/DST arithmetic).
const DAY_MS: i64 = 86_400_000;

/// The oldest `added_at` that may stay on the line for `policy` at `now_ms`.
///
/// [`KeepForDays::Forever`] yields `None` (no card ever expires). A card is expired when
/// `added_at < cutoff`, so a card exactly `N` days old is still inside "the last N days"; the
/// saturating arithmetic keeps a nonsensical clock from panicking.
pub fn cutoff_ms(policy: KeepForDays, now_ms: i64) -> Option<i64> {
    match policy {
        KeepForDays::Forever => None,
        KeepForDays::Days(days) => Some(now_ms.saturating_sub(i64::from(days) * DAY_MS)),
    }
}

/// Removes every card older than `policy` allows; returns the removed rows oldest first.
///
/// `items` must be sorted by `added_at` ascending (the invariant of every mutator), so the returned
/// rows are exactly the line's oldest prefix. Pure: no file, thumbnail or watched path is touched.
fn drop_expired(items: &mut Vec<Item>, policy: KeepForDays, now_ms: i64) -> Vec<Item> {
    let Some(cutoff) = cutoff_ms(policy, now_ms) else {
        return Vec::new();
    };
    let mut expired = Vec::new();
    let mut kept = Vec::with_capacity(items.len());
    for item in items.drain(..) {
        if item.added_at < cutoff {
            expired.push(item);
        } else {
            kept.push(item);
        }
    }
    *items = kept;
    expired
}

/// One side of the target card a side take-down removes (`take_down_side`).
///
/// The line is ordered by `added_at` ascending (design spec §4.5: the oldest card hangs left, the
/// newest right), so `before` is the older prefix and `after` the newer suffix around the target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    /// Cards older than (to the left of) the target.
    Before,
    /// Cards newer than (to the right of) the target.
    After,
}

impl Side {
    /// Parses the wire value of the `take_down_side` command; only `"before"`/`"after"` are valid.
    pub fn parse(side: &str) -> Option<Self> {
        match side {
            "before" => Some(Self::Before),
            "after" => Some(Self::After),
            _ => None,
        }
    }

    /// The stable wire name (also used in the command log line).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Before => "before",
            Self::After => "after",
        }
    }
}

/// Ids of the cards strictly on `side` of `target_id`, in line order.
///
/// `None` when the target is not on the line: the command treats that as an idempotent no-op, the
/// same way `take_down` handles an already-gone card. The target itself is never returned and the
/// opposite side is never included. Pure, so the selection contract is unit-tested without a state.
pub fn side_ids(items: &[Item], target_id: &str, side: Side) -> Option<Vec<String>> {
    let index = items.iter().position(|item| item.id == target_id)?;
    let (start, end) = match side {
        Side::Before => (0, index),
        Side::After => (index + 1, items.len()),
    };
    Some(items[start..end].iter().map(|item| item.id.clone()).collect())
}

/// The snapshot+emit gate behind [`AppState::broadcast`].
///
/// Holding this lock across "take snapshot, emit" is what makes event order match revision order:
/// two folder watcher threads and the IPC thread cannot snapshot revision N, interleave, and emit
/// it after revision N+1. Equal revisions are not re-emitted because a mutation bumps the revision,
/// so an unchanged revision carries an identical full state.
#[derive(Debug, Default)]
struct BroadcastGate {
    /// Revision of the last emitted snapshot (`None` before the first broadcast).
    last_revision: Option<u64>,
}

/// The managed application state (items + settings + the data directory they persist to).
pub struct AppState {
    inner: Mutex<Inner>,
    /// Dedicated mutex for the snapshot+emit section; never held across storage I/O.
    broadcast: Mutex<BroadcastGate>,
}

impl AppState {
    /// Loads settings and items from `dir`, pruning items whose files are gone.
    ///
    /// The pruned list is written back immediately (that is the startup normalization), so the
    /// next launch starts from a state file that only references existing files. A corrupt or
    /// missing settings/state file falls back to defaults instead of failing startup.
    pub fn load(dir: PathBuf) -> Self {
        let settings = settings::load(&dir);
        let (items, removed) = store::load_and_prune(&dir);
        let mut inner = Inner {
            dir,
            items,
            settings,
            revision: 0,
            capture_origins: VecDeque::new(),
        };
        // Startup normalization enforces the persisted cap as well (a `keepOnLine` reduction that
        // was persisted right before a crash may have left a larger `state.json` behind). The
        // rewritten list is the same state-minimizing pass as the file-existence prune.
        let dropped = inner.enforce_retention();
        if !removed.is_empty() || dropped > 0 {
            if let Err(err) = store::save_atomic(&inner.dir, &inner.items) {
                log::error!("app state: failed to persist the normalized item list: {err:#}");
            }
        }
        log::info!(
            "app state loaded: {} item(s) from {}",
            inner.items.len(),
            inner.dir.display()
        );
        Self {
            inner: Mutex::new(inner),
            broadcast: Mutex::new(BroadcastGate::default()),
        }
    }

    /// Number of items currently on the line (startup logging).
    pub fn item_count(&self) -> usize {
        self.lock().items.len()
    }

    /// Clones the current state for IPC / event payloads.
    pub fn snapshot(&self) -> PublicState {
        self.lock().snapshot()
    }

    /// Clones the current settings only (cheap; used by the 250 ms full-screen poller).
    ///
    /// The poller only needs `showOverFullScreen` to evaluate the pinned override; cloning the
    /// whole item list every tick would be wasteful once the line is long.
    pub fn settings(&self) -> Settings {
        self.lock().settings.clone()
    }

    /// Returns a clone of the item with `id`, or `None` when it already left the line.
    ///
    /// Commands resolve a card from the current state instead of trusting a snapshot the frontend
    /// held while the line changed (drag-out, take-down, a watcher prune).
    pub fn item(&self, id: &str) -> Option<Item> {
        self.lock()
            .items
            .iter()
            .find(|item| item.id == id)
            .cloned()
    }

    /// Records the selection rect of a finished capture for its fly-in (spec §4.6 C).
    ///
    /// The rect is consumed by `take_capture_origin` right after the new-item `state` snapshot; a
    /// flight that never asks for it expires after [`CAPTURE_ORIGIN_TTL`], and at most
    /// [`CAPTURE_ORIGIN_MAX`] pending rects are kept (bounded by design).
    pub fn record_capture_origin(&self, id: &str, rect: PhysicalRect) {
        self.record_capture_origin_at(id, rect, Instant::now());
    }

    /// Takes (removes) the capture rect for `id`; a miss is normal (non-capture items).
    pub fn take_capture_origin(&self, id: &str) -> Option<PhysicalRect> {
        self.take_capture_origin_at(id, Instant::now())
    }

    /// [`Self::record_capture_origin`] with an explicit clock (deterministic TTL unit tests).
    fn record_capture_origin_at(&self, id: &str, rect: PhysicalRect, now: Instant) {
        let mut inner = self.lock();
        inner.prune_capture_origins(now);
        // A re-capture for the same id replaces the stale rect instead of duplicating it.
        inner.capture_origins.retain(|origin| origin.id != id);
        inner.capture_origins.push_back(CaptureOrigin {
            id: id.to_string(),
            rect,
            at: now,
        });
        while inner.capture_origins.len() > CAPTURE_ORIGIN_MAX {
            inner.capture_origins.pop_front();
        }
    }

    /// [`Self::take_capture_origin`] with an explicit clock (deterministic TTL unit tests).
    fn take_capture_origin_at(&self, id: &str, now: Instant) -> Option<PhysicalRect> {
        let mut inner = self.lock();
        inner.prune_capture_origins(now);
        let index = inner
            .capture_origins
            .iter()
            .position(|origin| origin.id == id)?;
        inner.capture_origins.remove(index).map(|origin| origin.rect)
    }

    /// Validates, applies and persists one `set_setting` write, returning the new snapshot.
    ///
    /// Validation failures leave both memory and disk untouched. A persistence failure is returned
    /// to the caller (the setting is already applied in memory and will be retried on the next
    /// change), because a silently lost preference is worse than a visible error.
    ///
    /// A reduced `keepOnLine` takes effect immediately (M14): the oldest cards leave the line and
    /// the trimmed list is persisted, with **one** revision bump for the whole write. Both files are
    /// written settings-first, and startup in [`AppState::load`] re-applies the cap, so a crash
    /// between the two writes cannot leave an over-cap line behind. No file is ever deleted.
    pub fn set_json(&self, key: &str, value: Value) -> anyhow::Result<PublicState> {
        let mut inner = self.lock();
        inner.settings.set_json(key, value)?;
        let dropped = inner.enforce_retention();
        inner.bump();
        settings::save_atomic(&inner.dir, &inner.settings)
            .with_context(|| format!("failed to persist setting {key}"))?;
        if dropped > 0 {
            store::save_atomic(&inner.dir, &inner.items)
                .context("failed to persist the retention-trimmed item list")?;
        }
        Ok(inner.snapshot())
    }

    /// Replaces the whole settings object and persists it (used for side-effect rollback).
    ///
    /// `actions::apply` calls this when a hard side effect (hotkey registration, autostart
    /// registration) failed after the new value had already been persisted: the previous settings
    /// object is written back so memory, disk and the live registration agree again. The revision
    /// is bumped because the full state changed twice. Like [`Self::set_json`], a restored
    /// `keepOnLine` cap is enforced on the current list (normally a no-op).
    pub fn restore_settings(&self, settings: Settings) -> anyhow::Result<PublicState> {
        let mut inner = self.lock();
        inner.settings = settings;
        let dropped = inner.enforce_retention();
        inner.bump();
        settings::save_atomic(&inner.dir, &inner.settings)
            .context("failed to persist the rolled-back settings")?;
        if dropped > 0 {
            store::save_atomic(&inner.dir, &inner.items)
                .context("failed to persist the retention-trimmed item list")?;
        }
        Ok(inner.snapshot())
    }

    /// Returns whether `path` is already hung on the line (used by the folder watchers).
    ///
    /// Windows paths are case-insensitive, and a watcher notification can differ from the stored
    /// path in drive-letter or component case, so the comparison folds ASCII case.
    pub fn contains_path(&self, path: &Path) -> bool {
        let needle = path.to_string_lossy();
        self.lock().items.iter().any(|item| {
            item.path == path || item.path.to_string_lossy().eq_ignore_ascii_case(&needle)
        })
    }

    /// Appends an item (already hashed/deduped by the caller), persists, returns the new snapshot.
    ///
    /// Applies the retention cap first (M14): a [`KeepOnLine::Limit`] keeps the newest `n` cards by
    /// dropping the oldest rows from the state only; the dropped files stay on disk. The whole
    /// push → sort → trim → persist runs under one lock, so two watcher threads cannot interleave an
    /// over-cap list and each accepted item is one revision.
    pub fn add_item(&self, item: Item) -> anyhow::Result<PublicState> {
        let mut inner = self.lock();
        inner.items.push(item);
        // Spec 4.5: ascending added_at (the newest hangs on the right).
        inner.items.sort_by_key(|item| item.added_at);
        inner.enforce_retention();
        inner.bump();
        store::save_atomic(&inner.dir, &inner.items).context("failed to persist the item list")?;
        Ok(inner.snapshot())
    }

    /// Removes the cards older than the `keepForDays` window, persists, returns them oldest first.
    ///
    /// The state half of the age sweep (`retention::sweep` owns the lifecycle): the removed rows'
    /// files are deliberately left on disk, so the caller can recycle the Hangline-owned ones after
    /// the lock is released. One sweep that removed something is one revision; an empty sweep is a
    /// no-op that neither rewrites `state.json` nor bumps the revision.
    pub fn trim_expired(&self, now_ms: i64) -> anyhow::Result<Vec<Item>> {
        let expired = {
            let mut inner = self.lock();
            let expired = inner.enforce_age_retention(now_ms);
            if expired.is_empty() {
                return Ok(expired);
            }
            inner.bump();
            store::save_atomic(&inner.dir, &inner.items)
                .context("failed to persist the age-trimmed item list")?;
            expired
        };
        // P2-7 parity with `prune`/`remove`: the cache files of rows that left the line go too.
        for item in &expired {
            crate::thumbnails::remove_for_id(&item.id);
        }
        Ok(expired)
    }

    /// Applies thumbnail results from the P2-7 startup backfill (by id) and persists when changed.
    ///
    /// Only `thumb_path` and the repaired `width`/`height` are copied: another thread may have
    /// removed the item or changed its path meanwhile, and the backfill must never
    /// resurrect/overwrite those fields.
    pub fn apply_thumb_updates(&self, updates: &[Item]) -> anyhow::Result<bool> {
        let mut inner = self.lock();
        let mut changed = false;
        for update in updates {
            let Some(existing) = inner.items.iter_mut().find(|item| item.id == update.id) else {
                continue;
            };
            if existing.thumb_path == update.thumb_path
                && existing.width == update.width
                && existing.height == update.height
            {
                continue;
            }
            existing.thumb_path = update.thumb_path.clone();
            existing.width = update.width;
            existing.height = update.height;
            changed = true;
        }
        if !changed {
            return Ok(false);
        }
        inner.bump();
        store::save_atomic(&inner.dir, &inner.items)
            .context("failed to persist the thumbnail backfill")?;
        Ok(true)
    }

    /// Replaces the item with the same id after an in-place file rewrite (P3 markup/trim).
    ///
    /// The caller rewrites the file first and then hands over the refreshed metadata (path, size,
    /// hash, thumbnail, `updated_at`). Identity is fixed: the id must be on the line and the path
    /// must match the stored row, otherwise the replacement could strand a card on a different
    /// file. On success the item is persisted before the new snapshot is returned; a missing id or
    /// a mismatched path is rejected before the revision is bumped.
    pub fn replace_item(&self, item: Item) -> anyhow::Result<PublicState> {
        let mut inner = self.lock();
        let Some(existing) = inner.items.iter_mut().find(|row| row.id == item.id) else {
            anyhow::bail!("cannot replace item {}: it is not on the line", item.id);
        };
        if existing.path != item.path {
            anyhow::bail!(
                "cannot replace item {}: path {:?} does not match {:?}",
                item.id,
                item.path,
                existing.path
            );
        }
        *existing = item;
        inner.bump();
        store::save_atomic(&inner.dir, &inner.items)
            .context("failed to persist the rewritten item")?;
        Ok(inner.snapshot())
    }

    /// Removes the item with `id` (if any), persists, and returns the removed item.
    #[allow(dead_code)] // Task 12/13 (take-down / drag-away) remove items.
    pub fn remove(&self, id: &str) -> anyhow::Result<Option<Item>> {
        // P2-7 review: all cache/file I/O happens after the state guard is dropped, so a slow disk
        // or a locked preview can never stall every other state read/write.
        let removed = {
            let mut inner = self.lock();
            let Some(index) = inner.items.iter().position(|item| item.id == id) else {
                return Ok(None);
            };
            let removed = inner.items.remove(index);
            inner.bump();
            store::save_atomic(&inner.dir, &inner.items)
                .context("failed to persist the item list")?;
            removed
        };
        // P2-7: an item that leaves the line must not leave its cached preview behind.
        crate::thumbnails::remove_for_id(id);
        Ok(Some(removed))
    }

    /// Removes every row whose id is in `ids` with **one** revision bump and **one** persist.
    ///
    /// The batch form behind the `take_down_side` command: the source files are deliberately left
    /// on disk (the same contract as `take_down(deleteFile = false)`), and the caller broadcasts the
    /// new snapshot exactly once. Ids that already left the line are ignored; when nothing is
    /// removed this is a no-op that neither rewrites `state.json` nor bumps the revision. Returns
    /// the removed rows in line order.
    pub fn remove_many(&self, ids: &[String]) -> anyhow::Result<Vec<Item>> {
        let removed = {
            let mut inner = self.lock();
            let mut kept = Vec::with_capacity(inner.items.len());
            let mut removed = Vec::with_capacity(ids.len().min(inner.items.len()));
            for item in inner.items.drain(..) {
                if ids.iter().any(|id| id == &item.id) {
                    removed.push(item);
                } else {
                    kept.push(item);
                }
            }
            inner.items = kept;
            if removed.is_empty() {
                return Ok(removed);
            }
            inner.bump();
            store::save_atomic(&inner.dir, &inner.items)
                .context("failed to persist the side take-down")?;
            removed
        };
        // P2-7 parity with `remove`/`prune`: the cache files of rows that left the line go too.
        for item in &removed {
            crate::thumbnails::remove_for_id(&item.id);
        }
        Ok(removed)
    }

    /// Drops items whose files disappeared (user deleted them in Explorer) and persists.
    ///
    /// Returns the removed ids; an unchanged list does not rewrite `state.json`.
    pub fn prune(&self) -> anyhow::Result<Vec<String>> {
        let removed = {
            let mut inner = self.lock();
            let removed = store::prune(&mut inner.items);
            if !removed.is_empty() {
                inner.bump();
                store::save_atomic(&inner.dir, &inner.items)
                    .context("failed to persist the pruned item list")?;
            }
            removed
        };
        // P2-7: delete the previews of rows that were removed from the line, outside the lock.
        for id in &removed {
            crate::thumbnails::remove_for_id(id);
        }
        Ok(removed)
    }

    /// Serializes "take the newest snapshot, emit it" for [`emit_state`].
    ///
    /// The callback runs while the dedicated broadcast mutex is held, so emitters cannot reorder
    /// full states: a caller that snapshotted revision N can never emit after one that broadcast
    /// revision N+1. An unchanged revision is not re-emitted (identical state; no mutation bumped
    /// it), but the current snapshot is still returned. The state mutex is only held during the
    /// clone, never across the emit.
    pub fn broadcast<F>(&self, emit: F) -> PublicState
    where
        F: FnOnce(&PublicState),
    {
        let mut gate = self.broadcast.lock().unwrap_or_else(PoisonError::into_inner);
        let snapshot = self.snapshot();
        if gate.last_revision == Some(snapshot.revision) {
            return snapshot;
        }
        gate.last_revision = Some(snapshot.revision);
        emit(&snapshot);
        snapshot
    }

    /// Locks the inner state; a poisoned lock (a panic in another thread) is recovered instead of
    /// permanently disabling the data layer.
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// Broadcasts the current full state as the `state` event; the single broadcast choke point.
///
/// Every emitter (folder watchers, `set_setting`, the startup initial emit) goes through here, so
/// snapshot + emit is serialized and the emitted revisions strictly increase. The tray menu is
/// refreshed after the broadcast (Task 15): it mirrors the settings, the item count and the panel
/// visibility, and this is the one place every state change passes through. Returns the snapshot
/// that was current at broadcast time (`None` when the state is not managed yet).
pub fn emit_state(app: &AppHandle) -> Option<PublicState> {
    let Some(state) = app.try_state::<AppState>() else {
        log::warn!("app state: not managed yet; cannot emit the state event");
        return None;
    };
    let snapshot = state.broadcast(|snapshot| emit_snapshot(app, snapshot));
    // The tray refresh is marshalled to the main thread by `tray::sync_state` (a no-op before the
    // tray is created and when nothing menu-relevant changed).
    crate::tray::sync_state(app);
    Some(snapshot)
}

/// Emits one snapshot as the `state` event (full state, design spec 4.4).
///
/// Private on purpose: callers must go through [`emit_state`], which holds the broadcast mutex
/// while snapshotting and emitting.
fn emit_snapshot(app: &AppHandle, snapshot: &PublicState) {
    if let Err(err) = app.emit("state", snapshot) {
        log::warn!("app state: failed to emit the state event: {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::HashSet;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Barrier};

    use serde_json::json;

    use crate::settings::{CardSize, KeepForDays, KeepOnLine, Settings, Theme};

    /// Scratch directory under `target/` (git-ignored); never touches `%LOCALAPPDATA%`.
    fn test_dir(tag: &str) -> PathBuf {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("app-state-tests")
            .join(format!("{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create the app-state test dir");
        dir
    }

    /// Creates a real file so the item survives the startup existence check.
    fn existing_file(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, b"item bytes").expect("create the test file");
        path
    }

    /// Builds a persisted item with deterministic times.
    fn item(id: &str, path: &Path, added_at: i64) -> Item {
        Item {
            id: id.to_string(),
            path: path.to_path_buf(),
            thumb_path: None,
            kind: crate::item::ItemKind::Capture,
            hash: 7,
            width: 10,
            height: 20,
            created_at: 1,
            added_at,
            updated_at: added_at,
        }
    }

    /// Adds `count` real-file items with strictly ascending `added_at`; returns `(id, path)` oldest
    /// first. The ids are deterministic 32-hex strings.
    fn add_items(
        state: &AppState,
        dir: &Path,
        count: usize,
        first_added_at: i64,
    ) -> Vec<(String, PathBuf)> {
        let mut rows = Vec::with_capacity(count);
        for index in 0..count {
            let id = format!("{first_added_at:08x}{index:024x}");
            let path = existing_file(dir, &format!("retention-{id}.png"));
            state
                .add_item(item(&id, &path, first_added_at + index as i64))
                .expect("add a retention fixture item");
            rows.push((id, path));
        }
        rows
    }

    #[test]
    fn snapshot_reflects_add_remove_and_prune() {
        let dir = test_dir("mutations");
        let state = AppState::load(dir.clone());

        let snapshot = state.snapshot();
        assert!(snapshot.items.is_empty());
        assert_eq!(snapshot.settings, Settings::default());
        assert_eq!(snapshot.version, env!("CARGO_PKG_VERSION"));

        // add: in-memory snapshot + persisted state.json
        let path = existing_file(&dir, "one.png");
        let one = Item::new(path, crate::item::ItemKind::Capture, 42, 10, 20);
        assert_eq!(
            state.add_item(one.clone()).expect("add item").items,
            vec![one.clone()]
        );
        assert_eq!(store::load(&dir), vec![one.clone()]);

        // remove: returns the removed item and persists the empty list
        assert_eq!(
            state.remove(&one.id).expect("remove item"),
            Some(one.clone())
        );
        assert!(state.snapshot().items.is_empty());
        assert!(store::load(&dir).is_empty());
        assert_eq!(state.remove("missing").expect("missing remove"), None);

        // prune: drops items whose files are gone and persists the result
        let pruned_path = existing_file(&dir, "two.png");
        let pruned = item("pruned", &pruned_path, 5);
        state.add_item(pruned.clone()).expect("add pruned item");
        fs::remove_file(&pruned_path).expect("delete the item file");
        assert_eq!(state.prune().expect("prune"), vec![pruned.id.clone()]);
        assert!(state.snapshot().items.is_empty());
        assert!(store::load(&dir).is_empty());

        let _ = fs::remove_dir_all(dir);
    }

    /// P3: an in-place rewrite (markup save / trim) replaces one row, persists it and bumps the
    /// revision exactly once; an unknown id or a different path is rejected without any change.
    #[test]
    fn replace_item_updates_the_row_persists_and_bumps_the_revision() {
        let dir = test_dir("replace-item");
        let state = AppState::load(dir.clone());
        let path = existing_file(&dir, "shot.png");
        let original = item("0123456789abcdef0123456789abcdef", &path, 10);
        state.add_item(original.clone()).expect("add the item");
        let before = state.snapshot();

        let mut rewritten = original.clone();
        rewritten.hash = 99;
        rewritten.width = 640;
        rewritten.height = 480;
        rewritten.updated_at = 1_700_000_000_900;
        // The persisted loader drops a thumbPath whose file is gone: create the cache file first.
        let thumb = dir.join("0123456789abcdef0123456789abcdef.png");
        fs::write(&thumb, b"thumb bytes").expect("create the thumbnail file");
        rewritten.thumb_path = Some(thumb);

        let after = state.replace_item(rewritten.clone()).expect("replace");
        assert_eq!(after.revision, before.revision + 1);
        assert_eq!(after.items, vec![rewritten.clone()]);
        assert_eq!(state.item(&rewritten.id), Some(rewritten.clone()));
        assert_eq!(
            store::load(&dir),
            vec![rewritten.clone()],
            "the rewritten row must be persisted before the snapshot is returned"
        );

        // Rejections: missing id and path mismatch must not mutate memory or bump the revision.
        let revision = state.snapshot().revision;
        let mut missing = original.clone();
        missing.id = "fedcba9876543210fedcba9876543210".to_string();
        let error = state
            .replace_item(missing)
            .expect_err("an id that is not on the line cannot be replaced");
        assert!(format!("{error:#}").contains("not on the line"), "{error:#}");
        let mut moved = original.clone();
        moved.path = dir.join("somewhere-else.png");
        let error = state
            .replace_item(moved)
            .expect_err("a different path must be rejected");
        assert!(format!("{error:#}").contains("does not match"), "{error:#}");
        assert_eq!(state.snapshot().revision, revision, "no bump for rejections");
        assert_eq!(state.item(&rewritten.id), Some(rewritten));

        let _ = fs::remove_dir_all(dir);
    }

    /// Task 15 review fix: every snapshot carries the Rust-resolved UI language, so the tray and
    /// the panel cannot disagree under `language: "system"`. The wire name is camelCase.
    #[test]
    fn snapshot_exposes_the_rust_resolved_ui_language() {
        let dir = test_dir("resolved-language");
        let state = AppState::load(dir.clone());
        assert_eq!(
            state.snapshot().resolved_language,
            crate::i18n::resolved_tag("system"),
            "the default system setting must use the single Rust resolution"
        );

        state
            .set_json("language", json!("zh-Hans"))
            .expect("explicit zh-Hans");
        assert_eq!(state.snapshot().resolved_language, "zh-Hans");
        state.set_json("language", json!("en")).expect("explicit en");
        assert_eq!(state.snapshot().resolved_language, "en");

        let value = serde_json::to_value(state.snapshot()).expect("serialize snapshot");
        assert_eq!(value["resolvedLanguage"], json!("en"));
        assert_eq!(
            value["settings"]["language"],
            json!("en"),
            "the original setting stays available for the language radio buttons"
        );

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn contains_path_ignores_case_and_reports_hung_items() {
        let dir = test_dir("contains-path");
        let state = AppState::load(dir.clone());
        let path = existing_file(&dir, "one.png");

        assert!(
            !state.contains_path(&path),
            "an empty line contains no paths"
        );
        state
            .add_item(item("one", &path, 1))
            .expect("add the item");
        assert!(state.contains_path(&path));
        assert!(
            !state.contains_path(&dir.join("other.png")),
            "a different path is not on the line"
        );

        // Windows paths are case-insensitive; a watcher notification may differ in case.
        let upper = PathBuf::from(path.to_string_lossy().to_uppercase());
        assert_ne!(upper, path, "the test path must change when upper-cased");
        assert!(state.contains_path(&upper));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn settings_changes_are_validated_persisted_and_snapshotted() {
        let dir = test_dir("settings");
        let state = AppState::load(dir.clone());

        let snapshot = state
            .set_json("watchClipboard", json!(false))
            .expect("valid setting");
        assert!(!snapshot.settings.watch_clipboard);
        assert!(!settings::load(&dir).watch_clipboard);
        assert!(!state.snapshot().settings.watch_clipboard);

        // A rejected write must not change the in-memory or persisted settings.
        let before = state.snapshot().settings;
        let err = state
            .set_json("nope", json!(1))
            .expect_err("unknown key must be rejected");
        assert!(
            format!("{err:#}").contains("unknown setting key: nope"),
            "unexpected error: {err:#}"
        );
        assert_eq!(state.snapshot().settings, before);
        assert_eq!(settings::load(&dir), before);

        // Enum/number values go through the same validation as the file.
        state.set_json("keepOnLine", json!(25)).expect("limit");
        assert_eq!(
            state.snapshot().settings.keep_on_line,
            KeepOnLine::Limit(25)
        );
        state.set_json("cardSize", json!("small")).expect("size");
        assert_eq!(state.snapshot().settings.card_size, CardSize::Small);
        state.set_json("theme", json!("dark")).expect("theme");
        assert_eq!(state.snapshot().settings.theme, Theme::Dark);
        assert!(state.set_json("cardSize", json!("huge")).is_err());

        let _ = fs::remove_dir_all(dir);
    }

    /// M14: the cap arithmetic is pure; `Fit` never caps and the limits keep the newest cards.
    #[test]
    fn retained_len_caps_only_fixed_limits() {
        assert_eq!(retained_len(KeepOnLine::Fit, 0), 0);
        assert_eq!(retained_len(KeepOnLine::Fit, 150), 150);
        assert_eq!(retained_len(KeepOnLine::Limit(100), 101), 100);
        assert_eq!(retained_len(KeepOnLine::Limit(25), 30), 25);
        assert_eq!(retained_len(KeepOnLine::Limit(25), 25), 25);
        assert_eq!(retained_len(KeepOnLine::Limit(25), 10), 10);
    }

    /// Age retention is a pure, day-granularity policy: `Forever` has no cutoff, and 7/30/90 days
    /// subtract exact 24 h days from the sweep clock (no calendar/DST dependency).
    #[test]
    fn age_retention_cutoff_only_applies_to_fixed_windows() {
        let now = 1_000_000_000_000_i64;
        assert_eq!(cutoff_ms(KeepForDays::Forever, now), None);
        assert_eq!(
            cutoff_ms(KeepForDays::Days(7), now),
            Some(now - 7 * 86_400_000)
        );
        assert_eq!(
            cutoff_ms(KeepForDays::Days(30), now),
            Some(now - 30 * 86_400_000)
        );
        assert_eq!(
            cutoff_ms(KeepForDays::Days(90), now),
            Some(now - 90 * 86_400_000)
        );
    }

    /// A card exactly N days old is still inside "the last N days" and stays; one millisecond older
    /// has expired. `Forever` never removes anything.
    #[test]
    fn drop_expired_uses_the_adding_time_and_keeps_the_boundary() {
        let dir = test_dir("age-drop");
        let now = 1_000_000_000_000_i64;
        let cutoff = now - 7 * 86_400_000;
        let mut items = vec![
            item("old", &existing_file(&dir, "old.png"), cutoff - 1),
            item("boundary", &existing_file(&dir, "boundary.png"), cutoff),
            item("fresh", &existing_file(&dir, "fresh.png"), cutoff + 1),
        ];
        let expired = drop_expired(&mut items, KeepForDays::Days(7), now);
        let expired_ids: Vec<&str> = expired.iter().map(|row| row.id.as_str()).collect();
        assert_eq!(expired_ids, vec!["old"]);
        let kept_ids: Vec<&str> = items.iter().map(|row| row.id.as_str()).collect();
        assert_eq!(kept_ids, vec!["boundary", "fresh"]);

        let mut untouched = items.clone();
        assert!(drop_expired(&mut untouched, KeepForDays::Forever, now).is_empty());
        assert_eq!(untouched, items, "Forever must not drop anything");

        let _ = fs::remove_dir_all(dir);
    }

    /// `trim_expired` is the state half of the age sweep: it drops the expired rows, persists the
    /// list and bumps the revision once. Files are untouched — recycling runs after the lock is
    /// released and is the caller's job (`retention::sweep`).
    #[test]
    fn trim_expired_removes_persists_and_keeps_the_files() {
        let dir = test_dir("age-trim");
        let state = AppState::load(dir.clone());
        let now = 1_000_000_000_000_i64;
        let cutoff = now - 7 * 86_400_000;
        let old = item("old", &existing_file(&dir, "old.png"), cutoff - 1);
        let boundary = item("boundary", &existing_file(&dir, "boundary.png"), cutoff);
        state.add_item(old.clone()).expect("add the old item");
        state.add_item(boundary.clone()).expect("add the boundary item");
        let before = state.set_json("keepForDays", json!(7)).expect("7 days");
        assert_eq!(
            before.items.len(),
            2,
            "the setting write alone must not trim; the sweep does"
        );

        let expired = state.trim_expired(now).expect("trim");
        assert_eq!(expired, vec![old.clone()]);
        let after = state.snapshot();
        assert_eq!(
            after.revision,
            before.revision + 1,
            "one sweep, one revision"
        );
        assert_eq!(after.items, vec![boundary.clone()]);
        assert_eq!(
            store::load(&dir),
            vec![boundary.clone()],
            "the trimmed list is persisted before the sweep returns"
        );
        assert!(old.path.exists(), "trim_expired only edits the state");

        let _ = fs::remove_dir_all(dir);
    }

    /// A sweep with nothing to drop is a no-op: no revision bump, no rewrite; `Forever` (the
    /// default) can never expire a card no matter how old it is.
    #[test]
    fn trim_expired_no_op_does_not_bump_the_revision() {
        let dir = test_dir("age-noop");
        let state = AppState::load(dir.clone());
        let now = 1_000_000_000_000_i64;
        state
            .add_item(item("fresh", &existing_file(&dir, "fresh.png"), now))
            .expect("add a fresh item");
        let before = state.set_json("keepForDays", json!(7)).expect("7 days");
        assert!(state.trim_expired(now).expect("trim").is_empty());
        assert_eq!(state.snapshot().revision, before.revision);

        let forever = state
            .set_json("keepForDays", json!("forever"))
            .expect("forever");
        assert!(state.trim_expired(i64::MAX).expect("trim forever").is_empty());
        assert_eq!(state.snapshot().revision, forever.revision);

        let _ = fs::remove_dir_all(dir);
    }

    /// M14: the 101st card with `keepOnLine: 100` drops only the oldest row; the newest hangs on
    /// the right, the persisted list stays at 100 and no file is deleted.
    #[test]
    fn adding_the_101st_card_drops_the_oldest_and_keeps_its_file() {
        let dir = test_dir("retention-add");
        let state = AppState::load(dir.clone());
        state.set_json("keepOnLine", json!(100)).expect("limit 100");
        let rows = add_items(&state, &dir, 100, 1_000);
        let before = state.snapshot();
        assert_eq!(before.items.len(), 100);

        let newest_path = existing_file(&dir, "retention-newest.png");
        let newest = item("ffffffffffffffffffffffffffffffff", &newest_path, 9_000);
        let after = state.add_item(newest.clone()).expect("add the 101st card");

        assert_eq!(after.items.len(), 100, "the cap is exact after the add");
        assert_eq!(after.revision, before.revision + 1, "one add is one revision");
        assert_eq!(
            after.items.first().map(|row| row.id.as_str()),
            Some(rows[1].0.as_str()),
            "only the oldest row was dropped"
        );
        assert_eq!(
            after.items.last().map(|row| row.id.as_str()),
            Some(newest.id.as_str()),
            "the newest card hangs on the right"
        );
        assert!(
            after
                .items
                .windows(2)
                .all(|pair| pair[0].added_at <= pair[1].added_at),
            "the kept rows stay ordered by added_at ascending"
        );
        assert!(!after.items.iter().any(|row| row.id == rows[0].0));
        assert!(
            rows[0].1.exists(),
            "retention never deletes the dropped card's file"
        );
        assert!(newest_path.exists());
        assert_eq!(store::load(&dir), after.items, "the trimmed list is persisted");

        let _ = fs::remove_dir_all(dir);
    }

    /// M14: lowering `keepOnLine` through the real `set_setting` path (`AppState::set_json`) trims
    /// the oldest cards immediately, persists one list and bumps the revision once; the kept rows
    /// are byte-for-byte unchanged and the dropped files stay on disk.
    #[test]
    fn reducing_keep_on_line_trims_persists_and_bumps_the_revision_once() {
        let dir = test_dir("retention-reduce");
        let state = AppState::load(dir.clone());
        let rows = add_items(&state, &dir, 30, 2_000);
        let before = state.snapshot();
        assert_eq!(before.items.len(), 30);

        let after = state.set_json("keepOnLine", json!(25)).expect("limit 25");
        assert_eq!(
            after.revision,
            before.revision + 1,
            "one setting write, one revision"
        );
        assert_eq!(after.items.len(), 25);
        assert_eq!(
            after.items,
            before.items[5..].to_vec(),
            "the kept rows (ids/paths/metadata) are untouched; only the oldest five left"
        );
        assert_eq!(settings::load(&dir).keep_on_line, KeepOnLine::Limit(25));
        assert_eq!(store::load(&dir), after.items);
        for (id, path) in &rows[..5] {
            assert!(path.exists(), "dropped card {id} must keep its file");
        }
        for (_, path) in &rows[5..] {
            assert!(path.exists());
        }

        // The exact boundary (25 cards, limit 25) and setting the same limit again are no-ops for
        // the line: nothing is dropped and no card changes.
        let boundary = state.set_json("keepOnLine", json!(25)).expect("same limit");
        assert_eq!(boundary.items, after.items);
        // Raising the limit restores nothing: the dropped cards are gone from the line.
        let raised = state.set_json("keepOnLine", json!(100)).expect("raise limit");
        assert_eq!(raised.items, after.items, "old cards do not come back");
        assert_eq!(raised.revision, after.revision + 2);

        let _ = fs::remove_dir_all(dir);
    }

    /// M14: `Fit` is a view policy, so the state keeps growing (there is no implicit cap).
    #[test]
    fn fit_never_trims_the_state() {
        let dir = test_dir("retention-fit");
        let state = AppState::load(dir.clone());
        assert_eq!(state.snapshot().settings.keep_on_line, KeepOnLine::Fit);
        let rows = add_items(&state, &dir, 130, 3_000);
        let snapshot = state.snapshot();
        assert_eq!(snapshot.items.len(), 130);
        assert_eq!(store::load(&dir).len(), 130);
        assert_eq!(
            snapshot.items.first().map(|row| row.id.as_str()),
            Some(rows[0].0.as_str())
        );
        assert_eq!(
            snapshot.items.last().map(|row| row.id.as_str()),
            Some(rows[129].0.as_str())
        );
        assert!(rows[0].1.exists());
        let _ = fs::remove_dir_all(dir);
    }

    /// M14: the persisted cap is also enforced at startup, so a crash between the settings and item
    /// writes (or a hand-edited file) cannot leave an over-cap line behind.
    #[test]
    fn load_trims_a_state_that_exceeds_the_persisted_limit() {
        let dir = test_dir("retention-load");
        let mut rows = Vec::new();
        for index in 0..30 {
            let id = format!("{:032x}", index + 1);
            let path = existing_file(&dir, &format!("load-{index}.png"));
            rows.push(item(&id, &path, 10 + index));
        }
        store::save_atomic(&dir, &rows).expect("seed state.json");
        fs::write(dir.join("settings.json"), br#"{"keepOnLine": 25}"#).expect("settings");

        let state = AppState::load(dir.clone());
        let snapshot = state.snapshot();
        assert_eq!(snapshot.items.len(), 25);
        assert_eq!(snapshot.items, rows[5..].to_vec(), "the newest 25 stay");
        assert_eq!(
            store::load(&dir),
            rows[5..].to_vec(),
            "the normalized list is written back"
        );
        for row in &rows[..5] {
            assert!(row.path.exists(), "a trimmed row keeps its file");
        }
        let _ = fs::remove_dir_all(dir);
    }

    /// A settings rollback to a smaller cap must not leave an over-cap line (defensive parity with
    /// `set_json`; the rollback path normally only restores hotkey/autostart/language values).
    #[test]
    fn rolling_back_to_a_smaller_limit_trims_the_state() {
        let dir = test_dir("retention-rollback");
        let state = AppState::load(dir.clone());
        let rows = add_items(&state, &dir, 30, 4_000);
        let before = state.snapshot();
        let mut smaller = before.settings.clone();
        smaller.keep_on_line = KeepOnLine::Limit(25);
        let after = state.restore_settings(smaller).expect("rollback");
        assert_eq!(after.items.len(), 25);
        assert_eq!(after.items, before.items[5..].to_vec());
        assert_eq!(after.revision, before.revision + 1);
        assert_eq!(settings::load(&dir).keep_on_line, KeepOnLine::Limit(25));
        assert_eq!(store::load(&dir), after.items);
        for (_, path) in &rows[..5] {
            assert!(path.exists());
        }
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn load_drops_and_rewrites_items_whose_files_are_gone() {
        let dir = test_dir("startup-prune");
        let kept = item("kept", &existing_file(&dir, "kept.png"), 10);
        let gone = item("gone", &dir.join("gone.png"), 20);
        store::save_atomic(&dir, &[kept.clone(), gone]).expect("seed state.json");

        let state = AppState::load(dir.clone());
        assert_eq!(state.snapshot().items, vec![kept.clone()]);
        assert_eq!(
            store::load(&dir),
            vec![kept],
            "the pruned list must be written back at startup"
        );
        let _ = fs::remove_dir_all(dir);
    }

    /// Task 13: the shell moved/deleted the dragged file; prune drops the card, bumps the
    /// revision and rewrites `state.json`, while an item whose file still exists stays.
    #[test]
    fn pruning_a_deleted_file_drops_it_bumps_the_revision_and_persists() {
        let dir = test_dir("drag-prune");
        let state = AppState::load(dir.clone());
        let dragged_path = existing_file(&dir, "dragged.png");
        let kept_path = existing_file(&dir, "kept.png");
        let dragged = item("dragged", &dragged_path, 1);
        let kept = item("kept", &kept_path, 2);

        state.add_item(dragged.clone()).expect("add the dragged item");
        state.add_item(kept.clone()).expect("add the kept item");
        let before = state.snapshot();
        assert_eq!(before.revision, 2);

        // The drag target (Explorer / Recycle Bin) owns the move/delete; Hangline only observes.
        fs::remove_file(&dragged_path).expect("simulate the shell move or delete");

        assert_eq!(state.prune().expect("prune after a drag"), vec![dragged.id]);
        let after = state.snapshot();
        assert_eq!(
            after.revision,
            before.revision + 1,
            "a prune that removes an item bumps the revision exactly once"
        );
        assert_eq!(after.items, vec![kept.clone()]);
        assert_eq!(
            store::load(&dir),
            vec![kept],
            "the pruned list is persisted immediately"
        );

        assert!(state.prune().expect("second prune").is_empty());
        assert_eq!(
            state.snapshot().revision,
            before.revision + 1,
            "an empty prune is a no-op and must not bump the revision"
        );

        let _ = fs::remove_dir_all(dir);
    }

    /// Task 15: rolling a rejected change back must restore memory *and* `settings.json` (the tray
    /// shows the restored value and the next launch starts from it).
    #[test]
    fn restore_settings_rolls_back_memory_disk_and_bumps_the_revision() {
        let dir = test_dir("restore");
        let state = AppState::load(dir.clone());
        let before = state.snapshot().settings;
        let changed = state
            .set_json("captureHotkey", json!("Ctrl+Shift+F9"))
            .expect("valid hotkey");
        assert_eq!(changed.settings.capture_hotkey, "Ctrl+Shift+F9");
        assert_eq!(settings::load(&dir).capture_hotkey, "Ctrl+Shift+F9");
        let revision_after_change = changed.revision;

        let restored = state
            .restore_settings(before.clone())
            .expect("rollback persists");
        assert_eq!(restored.settings, before);
        assert_eq!(settings::load(&dir), before);
        assert_eq!(
            restored.revision,
            revision_after_change + 1,
            "a rollback is a second full-state change"
        );
        assert_eq!(state.snapshot().revision, restored.revision);

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn load_tolerates_corrupt_settings_and_state_files() {
        let dir = test_dir("corrupt");
        fs::write(dir.join("settings.json"), b"{ broken").expect("corrupt settings");
        fs::write(dir.join("state.json"), b"[] broken").expect("corrupt state");

        let state = AppState::load(dir);
        assert_eq!(state.snapshot().settings, Settings::default());
        assert!(state.snapshot().items.is_empty());
    }

    #[test]
    fn revision_increments_on_every_mutation_and_not_on_rejections() {
        let dir = test_dir("revision");
        let state = AppState::load(dir.clone());
        assert_eq!(state.snapshot().revision, 0, "a fresh state starts at 0");

        // add: one item, one revision
        let one = Item::new(
            existing_file(&dir, "one.png"),
            crate::item::ItemKind::Capture,
            1,
            10,
            20,
        );
        assert_eq!(state.add_item(one.clone()).expect("add item").revision, 1);

        // rejected setting: validation fails before the mutation, revision untouched
        assert!(state.set_json("nope", json!(true)).is_err());
        assert_eq!(state.snapshot().revision, 1);

        // accepted setting: persisted, revision advanced
        state
            .set_json("watchClipboard", json!(false))
            .expect("valid setting");
        assert_eq!(state.snapshot().revision, 2);

        // removing a missing id is not a change
        assert_eq!(state.remove("missing").expect("missing remove"), None);
        assert_eq!(state.snapshot().revision, 2);

        // removing an existing item is a change
        assert_eq!(
            state.remove(&one.id).expect("remove item"),
            Some(one),
            "the removed item is returned"
        );
        assert_eq!(state.snapshot().revision, 3);

        // pruning nothing is not a change
        assert!(state.prune().expect("empty prune").is_empty());
        assert_eq!(state.snapshot().revision, 3);

        // pruning a vanished file is a change
        let gone_path = existing_file(&dir, "gone.png");
        let gone = item("gone", &gone_path, 5);
        state.add_item(gone).expect("add pruned item");
        assert_eq!(state.snapshot().revision, 4);
        fs::remove_file(&gone_path).expect("delete the item file");
        assert_eq!(state.prune().expect("prune"), vec!["gone".to_string()]);
        assert_eq!(state.snapshot().revision, 5);

        let _ = fs::remove_dir_all(dir);
    }

    /// The choke point must never emit an older revision after a newer one, and must never emit a
    /// duplicate revision (equal revisions are identical full states).
    #[test]
    fn broadcast_revisions_are_monotonic_under_concurrent_mutations() {
        let dir = test_dir("broadcast");
        let state = Arc::new(AppState::load(dir.clone()));
        let emitted = Arc::new(Mutex::new(Vec::<(u64, usize)>::new()));
        let threads = 8;
        let barrier = Arc::new(Barrier::new(threads));

        let mut handles = Vec::with_capacity(threads);
        for index in 0..threads {
            let state = Arc::clone(&state);
            let emitted = Arc::clone(&emitted);
            let barrier = Arc::clone(&barrier);
            let path = existing_file(&dir, &format!("thread-{index}.png"));
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                state
                    .add_item(Item::new(
                        path,
                        crate::item::ItemKind::Capture,
                        index as u64,
                        1,
                        1,
                    ))
                    .expect("concurrent add");
                state.broadcast(|snapshot| {
                    emitted
                        .lock()
                        .expect("emitted revisions")
                        .push((snapshot.revision, snapshot.items.len()));
                });
            }));
        }
        for handle in handles {
            handle.join().expect("broadcast thread");
        }

        let emitted = emitted.lock().expect("emitted revisions");
        assert!(!emitted.is_empty(), "at least one snapshot is emitted");
        assert!(
            emitted.windows(2).all(|pair| pair[0].0 < pair[1].0),
            "revisions must strictly increase: {emitted:?}"
        );
        let unique: HashSet<u64> = emitted.iter().map(|(revision, _)| *revision).collect();
        assert_eq!(
            unique.len(),
            emitted.len(),
            "no snapshot is emitted twice: {emitted:?}"
        );
        assert_eq!(
            emitted.last().map(|(revision, _)| *revision),
            Some(threads as u64),
            "the last emitted snapshot carries the final revision"
        );
        for (revision, item_count) in emitted.iter() {
            assert_eq!(
                *item_count as u64, *revision,
                "revision and item list are snapshotted atomically"
            );
        }
        assert_eq!(state.snapshot().revision, threads as u64);

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn capture_origin_is_taken_once_and_survives_until_taken() {
        let dir = test_dir("capture-origin");
        let state = AppState::load(dir.clone());
        let rect = PhysicalRect {
            x: 10,
            y: 20,
            w: 300,
            h: 200,
        };
        state.record_capture_origin("item-a", rect);
        assert_eq!(state.take_capture_origin("item-a"), Some(rect));
        // Second take is a miss: the rect is consumed exactly once.
        assert_eq!(state.take_capture_origin("item-a"), None);
        // Unknown ids and non-capture items are normal misses.
        assert_eq!(state.take_capture_origin("item-b"), None);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn capture_origins_expire_after_thirty_seconds() {
        let dir = test_dir("capture-origin-ttl");
        let state = AppState::load(dir.clone());
        let rect = PhysicalRect {
            x: 0,
            y: 0,
            w: 10,
            h: 10,
        };
        let now = Instant::now();
        state.record_capture_origin_at("fresh", rect, now);
        state.record_capture_origin_at("stale", rect, now - Duration::from_secs(31));
        // The stale entry is pruned by the lookup; the fresh one is still available.
        assert_eq!(state.take_capture_origin_at("stale", now), None);
        assert_eq!(state.take_capture_origin_at("fresh", now), Some(rect));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn capture_origin_cap_keeps_only_the_newest_eight() {
        let dir = test_dir("capture-origin-cap");
        let state = AppState::load(dir.clone());
        let now = Instant::now();
        for index in 0..CAPTURE_ORIGIN_MAX + 3 {
            let id = format!("item-{index}");
            state.record_capture_origin_at(
                &id,
                PhysicalRect {
                    x: index as i32,
                    y: 0,
                    w: 1,
                    h: 1,
                },
                now,
            );
        }
        // The three oldest entries were evicted; the newest eight remain.
        for index in 0..3 {
            assert_eq!(state.take_capture_origin_at(&format!("item-{index}"), now), None);
        }
        for index in 3..CAPTURE_ORIGIN_MAX + 3 {
            assert_eq!(
                state.take_capture_origin_at(&format!("item-{index}"), now),
                Some(PhysicalRect {
                    x: index as i32,
                    y: 0,
                    w: 1,
                    h: 1,
                })
            );
        }
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn recording_the_same_id_replaces_the_previous_rect() {
        let dir = test_dir("capture-origin-replace");
        let state = AppState::load(dir.clone());
        let now = Instant::now();
        state.record_capture_origin_at(
            "item-a",
            PhysicalRect {
                x: 1,
                y: 1,
                w: 1,
                h: 1,
            },
            now,
        );
        let updated = PhysicalRect {
            x: 2,
            y: 2,
            w: 2,
            h: 2,
        };
        state.record_capture_origin_at("item-a", updated, now);
        assert_eq!(state.take_capture_origin_at("item-a", now), Some(updated));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn thumbnail_updates_apply_by_id_repair_dimensions_and_persist() {
        // P2-7 backfill contract: only thumb_path/width/height change, by id; a missing id is a
        // no-op, an identical update reports "no change" (no revision bump / no write), and a
        // repaired 0×0 dimension is accepted.
        let dir = test_dir("thumb-updates");
        let source = existing_file(&dir, "shot.png");
        let state = AppState::load(dir.clone());
        let mut item = item("0123456789abcdef0123456789abcdef", &source, 1);
        item.width = 0;
        item.height = 0;
        state.add_item(item.clone()).expect("add item");
        let revision_before = state.snapshot().revision;

        let mut updated = state.item(&item.id).expect("stored item");
        // The persisted path must point at a real file: `store::load` drops dangling thumb paths.
        fs::write(dir.join("thumb.png"), b"thumb bytes").expect("create the thumb file");
        updated.thumb_path = Some(dir.join("thumb.png"));
        updated.width = 3840;
        updated.height = 2160;
        assert!(state.apply_thumb_updates(&[updated.clone()]).expect("apply"));
        let stored = state.item(&item.id).expect("updated item");
        assert_eq!(stored.thumb_path, updated.thumb_path);
        assert_eq!((stored.width, stored.height), (3840, 2160));
        assert!(state.snapshot().revision > revision_before);
        let persisted = store::load(&dir);
        assert_eq!(persisted[0].thumb_path, updated.thumb_path);
        assert_eq!((persisted[0].width, persisted[0].height), (3840, 2160));

        // Identical update: no change and no persisted write.
        assert!(!state.apply_thumb_updates(&[updated.clone()]).expect("no-op"));
        // Unknown id: no-op.
        let mut missing = updated.clone();
        missing.id = "fedcba9876543210fedcba9876543210".to_string();
        assert!(!state.apply_thumb_updates(&[missing]).expect("missing id"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn remove_deletes_cached_thumb_after_state_guard_drops() {
        // P2-7 review: removal deletes the preview outside the state guard; the test cache-dir seam
        // keeps this test away from the real `%LOCALAPPDATA%`.
        let dir = test_dir("remove-thumb");
        let thumbs = dir.join("thumbs");
        fs::create_dir_all(&thumbs).expect("thumb dir");
        let _cache_guard = crate::thumbnails::use_test_cache_dir(thumbs.clone());
        let source = existing_file(&dir, "shot.png");
        let id = "0123456789abcdef0123456789abcdef";
        let mut stored = item(id, &source, 1);
        stored.thumb_path = Some(thumbs.join(format!("{id}.png")));
        fs::write(thumbs.join(format!("{id}.png")), b"thumb bytes").expect("thumb");
        let state = AppState::load(dir.clone());
        state.add_item(stored).expect("add item");

        let removed = state.remove(id).expect("remove item").expect("item existed");
        assert_eq!(removed.id, id);
        assert!(!thumbs.join(format!("{id}.png")).exists(), "preview deleted");
        assert!(state.item(id).is_none());
        let _ = fs::remove_dir_all(dir);
    }

    /// Builds one card per id, oldest first (the line's `added_at` ascending order).
    fn line(ids: &[&str]) -> Vec<Item> {
        ids.iter()
            .enumerate()
            .map(|(index, id)| item(id, Path::new("line.png"), index as i64))
            .collect()
    }

    /// `side_ids` around a middle target: the older prefix for `before`, the newer suffix for
    /// `after`; the target card itself is never in the selection.
    #[test]
    fn side_ids_selects_the_older_or_newer_side_around_the_target() {
        let items = line(&["a", "b", "c", "d", "e"]);
        assert_eq!(
            side_ids(&items, "c", Side::Before),
            Some(vec!["a".to_string(), "b".to_string()])
        );
        assert_eq!(
            side_ids(&items, "c", Side::After),
            Some(vec!["d".to_string(), "e".to_string()])
        );
    }

    /// The first and last cards have exactly one empty side; the cards on the other side all stay
    /// eligible for removal.
    #[test]
    fn side_ids_at_the_line_ends_leaves_the_boundary_side_empty() {
        let items = line(&["a", "b", "c", "d", "e"]);
        assert_eq!(side_ids(&items, "a", Side::Before), Some(Vec::new()));
        assert_eq!(
            side_ids(&items, "a", Side::After),
            Some(vec![
                "b".to_string(),
                "c".to_string(),
                "d".to_string(),
                "e".to_string()
            ])
        );
        assert_eq!(
            side_ids(&items, "e", Side::Before),
            Some(vec![
                "a".to_string(),
                "b".to_string(),
                "c".to_string(),
                "d".to_string()
            ])
        );
        assert_eq!(side_ids(&items, "e", Side::After), Some(Vec::new()));
    }

    /// A target that already left the line has no side (the command then treats it as a no-op).
    #[test]
    fn side_ids_returns_none_when_the_target_is_not_on_the_line() {
        let items = line(&["a", "b"]);
        assert_eq!(side_ids(&items, "gone", Side::Before), None);
        assert_eq!(side_ids(&items, "gone", Side::After), None);
        assert_eq!(side_ids(&[], "a", Side::Before), None);
    }

    /// Only the two documented wire names parse; everything else is rejected.
    #[test]
    fn side_parse_accepts_only_before_and_after() {
        assert_eq!(Side::parse("before"), Some(Side::Before));
        assert_eq!(Side::parse("after"), Some(Side::After));
        assert_eq!(Side::Before.as_str(), "before");
        assert_eq!(Side::After.as_str(), "after");
        for invalid in ["", "Before", "AFTER", "left", "right", "after ", " before"] {
            assert_eq!(Side::parse(invalid), None, "must reject {invalid:?}");
        }
    }

    /// The side take-down removes exactly the given rows with one revision bump and one persist;
    /// the source files stay on disk, and an empty/stale id list is a no-op.
    #[test]
    fn remove_many_removes_the_batch_with_one_revision_and_one_persist() {
        let dir = test_dir("remove-many");
        let state = AppState::load(dir.clone());
        let mut rows = Vec::new();
        for index in 0..4i64 {
            let id = format!("side-{index}");
            let path = existing_file(&dir, &format!("side-{index}.png"));
            state
                .add_item(item(&id, &path, index))
                .expect("add a side fixture item");
            rows.push((id, path));
        }
        let before = state.snapshot();
        let removed = state
            .remove_many(&[rows[0].0.clone(), rows[2].0.clone()])
            .expect("remove the batch");
        assert_eq!(removed.len(), 2);
        let after = state.snapshot();
        assert_eq!(
            after.revision,
            before.revision + 1,
            "one batch is exactly one revision bump"
        );
        assert_eq!(
            after
                .items
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            vec!["side-1", "side-3"]
        );
        assert_eq!(
            store::load(&dir),
            after.items,
            "the batch is persisted once (the .json matches the state)"
        );
        // Never deletes files: the source files of the removed cards are still on disk.
        assert!(rows[0].1.is_file());
        assert!(rows[2].1.is_file());

        // A stale id or an empty batch changes nothing (no revision bump, no rewrite).
        let unchanged = state.snapshot();
        assert!(state.remove_many(&[]).expect("empty batch").is_empty());
        assert!(state
            .remove_many(&[rows[0].0.clone()])
            .expect("stale id")
            .is_empty());
        assert_eq!(state.snapshot(), unchanged);

        let _ = fs::remove_dir_all(dir);
    }
}
