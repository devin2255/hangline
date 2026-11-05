//! Content-hash dedupe shared by the folder watchers, the clipboard watcher and the capture
//! overlay.
//!
//! Design spec 3.2: every new screenshot/recording is hashed with BLAKE3 (truncated to 64 bits) and
//! compared against a small LRU of recent hashes. The same picture arriving through two sources
//! (our own capture + the system screenshot folder, or the clipboard + a watched folder) must hang
//! on the line only once.
//!
//! Two structures share one mutex:
//! - the 20-entry LRU of recently seen content hashes (spec 3.2 rule 1), used by every watcher and
//!   clipboard flow; and
//! - the pending-claim set (spec 3.2 rule 2): our own capture records the hash *before* the file
//!   lands, and a watcher that sees the same hash within [`CLAIM_TTL`] (5 s) consumes the claim and
//!   drops the duplicate. An expired claim is ignored/cleaned; the LRU and
//!   `AppState::contains_path` remain the durable guards.
//!
//! [`Deduper`] is a single process-wide value managed by Tauri; it is internally synchronized so
//! the watcher threads, the clipboard message window and the capture command can all share it.
//!
//! # Task 16 own-capture contract (spec 3.2 rule 2)
//!
//! The exact order that keeps the claim and the folder watcher consistent is:
//!
//! 1. encode the PNG to its temporary name, `<final>.png.tmp`, in
//!    `%LOCALAPPDATA%\Hangline\Screenshots`;
//! 2. hash the *exact bytes* of that temp file with [`hash_file`] — the same BLAKE3 digest
//!    truncated to the first 8 bytes (little-endian `u64`) that the watcher computes; a rename does
//!    not change the bytes, so both sides hash the same content;
//! 3. [`Deduper::remember`] the hash: the 5 s claim exists *before* the file is visible under its
//!    final name;
//! 4. rename the temp file to its final `Hangline yyyy-MM-dd HH.mm.ss.png` name;
//! 5. `AppState::add_item` the persisted item.
//!
//! The watcher's candidate filter ignores the `.png.tmp` file, and its event fires on the rename.
//! If it fires before step 5 the claim suppresses it; if it fires after step 5 the claim (or the
//! LRU recorded by consuming it) still suppresses the duplicate. Once the claim expires,
//! `AppState::contains_path` remains the durable guard for the already-hung path.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

/// Number of content hashes remembered by the default [`Deduper`] (design spec 3.2: "最近 20 条").
pub const LRU_CAPACITY: usize = 20;

/// How long an own-capture claim suppresses the same content from another source.
///
/// Design spec 3.2 rule 2: B/C watchers seeing the claimed hash "within 5 s" drop it.
pub const CLAIM_TTL: Duration = Duration::from_secs(5);

/// The mutex-protected dedupe state.
#[derive(Debug, Default)]
struct Inner {
    /// Recently seen content hashes, most recently used last (at most `capacity`).
    hashes: Vec<u64>,
    /// Pending own-capture claims: hash -> instant the claim was recorded/refreshed.
    claims: HashMap<u64, Instant>,
}

/// Remembers the most recent content hashes and claims the ones an own capture is about to save.
///
/// Every hash lives in the LRU; a claim adds 5 s of suppression on top. Both live behind one mutex:
/// every operation is cheap (at most 20 LRU entries plus the claims of the last 5 s) and a poisoned
/// lock is recovered rather than disabling dedupe after an unrelated panic.
#[derive(Debug)]
pub struct Deduper {
    capacity: usize,
    claim_ttl: Duration,
    inner: Mutex<Inner>,
}

impl Default for Deduper {
    fn default() -> Self {
        Self::with_capacity(LRU_CAPACITY)
    }
}

impl Deduper {
    /// Creates a deduper keeping at most `capacity` hashes (a capacity of 0 dedupes nothing).
    pub fn with_capacity(capacity: usize) -> Self {
        Self::with_capacity_and_ttl(capacity, CLAIM_TTL)
    }

    /// Creates a deduper with an explicit claim TTL (tests use a short one; production uses 5 s).
    pub fn with_capacity_and_ttl(capacity: usize, claim_ttl: Duration) -> Self {
        Self {
            capacity,
            claim_ttl,
            inner: Mutex::new(Inner::default()),
        }
    }

    /// Returns `true` when `hash` is new (and remembers it), `false` when it was already seen.
    ///
    /// A pending claim is consulted first: within [`CLAIM_TTL`] of [`Self::remember`] the hash is
    /// reported as already known, the claim is consumed (exactly one event is suppressed) and the
    /// hash is recorded in the LRU. Expired claims are dropped and suppress nothing.
    ///
    /// A repeated hash refreshes its recency, so the LRU always holds the 20 *most recently
    /// checked* contents (that is what "within the LRU window" means for the watchers).
    pub fn check_or_insert(&self, hash: u64) -> bool {
        if self.capacity == 0 {
            return true;
        }
        let mut inner = self.lock();
        self.drop_expired_claims(&mut inner);
        if inner.claims.remove(&hash).is_some() {
            // An own capture already saved this content: consume the claim so it suppresses
            // exactly one watcher/clipboard event, and keep the hash in the durable LRU.
            Self::refresh_or_insert(&mut inner.hashes, hash, self.capacity);
            return false;
        }
        if Self::refresh(&mut inner.hashes, hash) {
            return false;
        }
        Self::insert(&mut inner.hashes, hash, self.capacity);
        true
    }

    /// Claims `hash` for an own capture about to be saved (spec 3.2 rule 2; see the module docs for
    /// the exact Task 16 order).
    ///
    /// The claim is recorded before the capture file gets its final name, so a watcher event that
    /// races `AppState::add_item` is suppressed. The hash also enters the LRU immediately, so after
    /// the claim expires the same content is still deduped while it stays in the LRU window.
    /// Claiming the same hash again refreshes the claim instead of creating a second one.
    #[allow(dead_code)] // Task 16 (own capture) claims hashes.
    pub fn remember(&self, hash: u64) {
        if self.capacity == 0 {
            return;
        }
        let mut inner = self.lock();
        self.drop_expired_claims(&mut inner);
        inner.claims.insert(hash, Instant::now());
        Self::refresh_or_insert(&mut inner.hashes, hash, self.capacity);
    }

    /// Forgets `hash` completely: drops a pending claim and removes the LRU entry.
    ///
    /// Used when persisting an item failed after the hash had already been claimed/inserted, so a
    /// retry of the same content is not mistaken for a duplicate.
    pub fn forget(&self, hash: u64) {
        let mut inner = self.lock();
        inner.claims.remove(&hash);
        inner.hashes.retain(|remembered| *remembered != hash);
    }

    /// Removes claims older than [`Self::claim_ttl`]; a claim never outlives its TTL window.
    fn drop_expired_claims(&self, inner: &mut Inner) {
        let now = Instant::now();
        inner
            .claims
            .retain(|_, claimed_at| now.saturating_duration_since(*claimed_at) < self.claim_ttl);
    }

    /// Moves `hash` to the most-recently-used end when it is already remembered.
    fn refresh(hashes: &mut Vec<u64>, hash: u64) -> bool {
        let Some(index) = hashes.iter().position(|remembered| *remembered == hash) else {
            return false;
        };
        let remembered = hashes.remove(index);
        hashes.push(remembered);
        true
    }

    /// Refreshes `hash` when present, otherwise inserts it as the newest entry.
    fn refresh_or_insert(hashes: &mut Vec<u64>, hash: u64, capacity: usize) {
        if !Self::refresh(hashes, hash) {
            Self::insert(hashes, hash, capacity);
        }
    }

    /// Pushes `hash` at the most-recently-used end, evicting the oldest entry when full.
    fn insert(hashes: &mut Vec<u64>, hash: u64, capacity: usize) {
        if hashes.len() >= capacity {
            hashes.remove(0);
        }
        hashes.push(hash);
    }

    /// Locks the dedupe state; a poisoned mutex (a panic in another thread) is recovered.
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Number of remembered LRU hashes (assertions in the LRU tests).
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.lock().hashes.len()
    }

    /// Number of pending (possibly expired) claims (assertions in the claim tests).
    #[cfg(test)]
    pub fn claim_count(&self) -> usize {
        self.lock().claims.len()
    }
}

/// Hashes a file's contents with BLAKE3 and truncates the digest to 64 bits.
///
/// The watchers hash the whole file (streamed, so large recordings do not need to fit in memory).
/// The truncated little-endian prefix is the dedupe key stored in [`crate::item::Item::hash`].
pub fn hash_file(path: &Path) -> std::io::Result<u64> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    hasher.update_reader(&mut file)?;
    Ok(truncate(hasher.finalize()))
}

/// Hashes bytes already in memory with the same BLAKE3-64 truncation as [`hash_file`].
///
/// The clipboard watcher encodes the PNG it is about to save, hashes those exact bytes, and then
/// writes them unchanged, so the in-memory hash equals `hash_file` of the saved file (which the
/// folder watcher will compute for that path).
pub fn hash_bytes(bytes: &[u8]) -> u64 {
    truncate(blake3::hash(bytes))
}

/// Truncates a BLAKE3 digest to its first 8 little-endian bytes (the dedupe key).
fn truncate(digest: blake3::Hash) -> u64 {
    let mut truncated = [0u8; 8];
    truncated.copy_from_slice(&digest.as_bytes()[..8]);
    u64::from_le_bytes(truncated)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;
    use std::path::PathBuf;
    use std::thread;

    /// Scratch directory under `target/` (git-ignored).
    fn test_dir(tag: &str) -> PathBuf {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("dedupe-tests")
            .join(format!("{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create the dedupe test dir");
        dir
    }

    #[test]
    fn check_or_insert_reports_only_the_first_time() {
        let deduper = Deduper::default();
        assert!(deduper.check_or_insert(11), "the first hash is new");
        assert!(!deduper.check_or_insert(11), "a repeat is not new");
        assert!(deduper.check_or_insert(12), "a different hash is new");
        assert_eq!(deduper.len(), 2);
    }

    #[test]
    fn lru_evicts_the_least_recently_used_hash() {
        let deduper = Deduper::with_capacity(3);
        for hash in [1, 2, 3] {
            assert!(deduper.check_or_insert(hash));
        }
        // Touching 1 makes 2 the least recently used entry.
        assert!(!deduper.check_or_insert(1));
        assert!(deduper.check_or_insert(4), "4 is new and evicts 2");
        assert_eq!(deduper.len(), 3);
        assert!(!deduper.check_or_insert(1), "1 was refreshed, not evicted");
        assert!(!deduper.check_or_insert(3), "3 is still remembered");
        assert!(
            deduper.check_or_insert(2),
            "an evicted hash is new again"
        );
    }

    #[test]
    fn remember_claims_a_hash_without_a_duplicate() {
        let deduper = Deduper::default();
        deduper.remember(7);
        assert_eq!(deduper.claim_count(), 1, "remember records a claim");
        assert!(
            !deduper.check_or_insert(7),
            "a remembered (claimed) hash must not be reported as new"
        );
        assert_eq!(deduper.claim_count(), 0, "the claim is consumed by the check");
        deduper.remember(7);
        assert_eq!(deduper.len(), 1, "remember is idempotent");
        assert_eq!(deduper.claim_count(), 1, "remember refreshes the claim");
    }

    #[test]
    fn a_claim_suppresses_exactly_the_next_check() {
        let deduper = Deduper::default();
        deduper.remember(21);
        assert_eq!(deduper.claim_count(), 1);
        assert!(
            !deduper.check_or_insert(21),
            "the claim suppresses the watcher event"
        );
        assert_eq!(deduper.claim_count(), 0, "the claim is consumed");
        assert_eq!(deduper.len(), 1, "the suppression records the hash in the LRU");

        // The LRU (not a lingering claim) suppresses the next repeat.
        assert!(!deduper.check_or_insert(21));
        assert_eq!(deduper.claim_count(), 0);
    }

    #[test]
    fn an_expired_claim_does_not_suppress() {
        let deduper = Deduper::with_capacity_and_ttl(1, Duration::from_millis(20));
        deduper.remember(4);
        assert_eq!(deduper.claim_count(), 1);

        // Evict the claimed hash from the one-entry LRU so only the claim could suppress it.
        assert!(deduper.check_or_insert(5));
        assert_eq!(deduper.len(), 1, "the LRU now holds 5, not 4");

        thread::sleep(Duration::from_millis(60));
        assert!(
            deduper.check_or_insert(4),
            "an expired claim must not suppress (the LRU no longer holds it)"
        );
        assert_eq!(deduper.claim_count(), 0, "the expired claim is cleaned up");
    }

    #[test]
    fn a_consumed_claim_does_not_suppress_twice() {
        let deduper = Deduper::with_capacity(1);
        deduper.remember(9);
        assert!(
            !deduper.check_or_insert(9),
            "the claim suppresses the first repeat"
        );
        assert!(deduper.check_or_insert(10), "a different hash is new");
        assert!(
            deduper.check_or_insert(9),
            "the hash was evicted and its claim was consumed; no lingering suppression"
        );
    }

    #[test]
    fn forget_clears_claims_and_lru_entries() {
        let deduper = Deduper::default();
        deduper.remember(31);
        assert_eq!(deduper.claim_count(), 1);
        assert_eq!(deduper.len(), 1);

        deduper.forget(31);
        assert_eq!(deduper.claim_count(), 0, "the pending claim is dropped");
        assert_eq!(deduper.len(), 0, "the LRU entry is dropped");
        assert!(
            deduper.check_or_insert(31),
            "a forgotten hash is new again (the failed persist is retryable)"
        );

        // Forgetting an unknown hash is a no-op.
        let len = deduper.len();
        deduper.forget(32);
        assert_eq!(deduper.len(), len);
    }

    #[test]
    fn the_claim_window_is_the_specified_five_seconds() {
        assert_eq!(
            CLAIM_TTL,
            Duration::from_secs(5),
            "spec 3.2 rule 2 pins the pending-claim window at 5 s"
        );
    }

    #[test]
    fn hash_file_is_64_bit_and_content_addressed() {
        let dir = test_dir("hash");
        let a = dir.join("a.bin");
        let b = dir.join("b.bin");
        let c = dir.join("c.bin");
        fs::write(&a, b"same bytes").expect("write a");
        fs::write(&b, b"same bytes").expect("write b");
        fs::write(&c, b"other bytes").expect("write c");

        let hash_a = hash_file(&a).expect("hash a");
        assert_eq!(hash_a, hash_file(&b).expect("hash b"), "content addressed");
        assert_ne!(hash_a, hash_file(&c).expect("hash c"));
        assert_eq!(hash_a.to_le_bytes().len(), 8, "64-bit hash");
        assert_eq!(
            hash_bytes(b"same bytes"),
            hash_a,
            "hashing bytes in memory equals hash_file on a file with those bytes"
        );

        let empty = dir.join("empty.bin");
        fs::write(&empty, b"").expect("write empty");
        assert_ne!(
            hash_file(&empty).expect("hash empty"),
            hash_a,
            "the empty file has its own hash"
        );

        assert!(hash_file(&dir.join("missing.bin")).is_err());
        let _ = fs::remove_dir_all(dir);
    }
}
