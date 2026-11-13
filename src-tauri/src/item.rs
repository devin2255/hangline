//! One item hung on the line: its identify, file path, provenance, content hash, and times.
//!
//! # Item identity without a uuid crate
//!
//! Design spec 4.5 says the id is a UUID v4, but Task 7 has a hard "no new dependencies" rule and
//! the `uuid` crate is not in `Cargo.toml`. The id is therefore an opaque 128-bit value derived
//! from `blake3` (already a dependency) over the current millisecond, the process id, a
//! process-wide sequence counter, the content hash and the path. That gives the same practical
//! properties as a v4 UUID for our use (unique, unpredictable enough, no coordination) with 32
//! lowercase hex characters. Callers must treat it as opaque and never parse it.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Length in hex characters of an [`Item`] id (128 bits).
pub const ITEM_ID_HEX_LEN: usize = 32;

/// Where an item came from (design spec 4.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ItemKind {
    /// A screenshot taken with Hangline's own capture overlay.
    Capture,
    /// A screenshot produced by the Windows screenshot tooling.
    SystemScreenshot,
    /// Image data copied to the clipboard.
    Clipboard,
    /// Image data dropped onto the line from another app (v1.5.0 drag-in; stored in Hangline's
    /// own screenshot directory so the card keeps working after the source file goes away).
    Dropped,
    /// A screen recording (`Videos\Captures`).
    Recording,
    /// An image found in a user-watched folder.
    WatchedFolder,
}

/// One item hung on the line (design spec 4.5).
///
/// Field names are serialized in camelCase to match the TypeScript `Item` interface. `path` is an
/// absolute path; `created_at` is the file creation time and `added_at` the time the item was hung,
/// both in milliseconds since the Unix epoch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    /// Opaque id (`32` hex characters, see the module docs).
    pub id: String,
    /// Absolute path of the image/video on disk.
    pub path: PathBuf,
    /// P2-7 cached thumbnail (`%LOCALAPPDATA%\Hangline\Thumbnails\<id>.png`). Recordings store
    /// their P3-5 first-frame preview here; absent only when the source cannot be decoded (or was
    /// persisted before P2-7), in which case the card shows its filename tile (recordings keep the
    /// play badge).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumb_path: Option<PathBuf>,
    /// Provenance of the item.
    pub kind: ItemKind,
    /// BLAKE3 hash truncated to 64 bits (dedupe key; Task 8 owns the hashing).
    pub hash: u64,
    /// Pixel width (0 for videos until the first frame is decoded).
    pub width: u32,
    /// Pixel height (0 for videos until the first frame is decoded).
    pub height: u32,
    /// File creation time in milliseconds since the Unix epoch.
    pub created_at: i64,
    /// Time the item was hung on the line in milliseconds since the Unix epoch.
    pub added_at: i64,
    /// Time the file was last rewritten in place by Hangline (markup save / trim), in milliseconds
    /// since the Unix epoch.
    ///
    /// `0` for items persisted before P3 (serde default: an old `state.json` loads unchanged); new
    /// items start at `added_at`. The frontend appends it to asset URLs (`?v=`) to defeat the
    /// WebView's byte cache after an in-place rewrite.
    #[serde(default)]
    pub updated_at: i64,
    /// User note (Wave 2 W2.1; empty for items persisted before Wave 2 — the serde default is the
    /// read-time migration, so an old `state.json` loads unchanged).
    #[serde(default)]
    pub note: String,
    /// User tags (Wave 2 W2.1). Normalized by `AppState::set_tags` (trimmed, de-duplicated, at most
    /// 20 non-empty tags of at most 32 characters); empty for pre-Wave-2 items.
    #[serde(default)]
    pub tags: Vec<String>,
}

impl Item {
    /// Creates an item for an existing file and fills in its identities/timestamps.
    ///
    /// `created_at` comes from the file metadata (falling back to the current time when the
    /// filesystem does not expose a creation time), and `added_at` is the current time. The id is
    /// derived from the path, content hash and a process-wide sequence, so two items for the same
    /// file still get distinct ids.
    #[allow(dead_code)] // Task 8 creates items; this task exercises it from the unit tests.
    pub fn new(path: PathBuf, kind: ItemKind, hash: u64, width: u32, height: u32) -> Self {
        let created_at = file_created_ms(&path).unwrap_or_else(now_ms);
        let added_at = now_ms();
        let id = new_id(&path, hash);
        Self {
            id,
            path,
            thumb_path: None,
            kind,
            hash,
            width,
            height,
            created_at,
            added_at,
            updated_at: added_at,
            note: String::new(),
            tags: Vec::new(),
        }
    }
}

/// Current wall-clock time in milliseconds since the Unix epoch.
#[allow(dead_code)] // Only the (Task 8) item-construction path needs it today.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// Creation time of `path` in milliseconds since the Unix epoch, when the filesystem knows it.
#[allow(dead_code)] // Used by `Item::new`, which Task 8 wires up.
fn file_created_ms(path: &Path) -> Option<i64> {
    let created = fs::metadata(path).ok()?.created().ok()?;
    let elapsed = created.duration_since(UNIX_EPOCH).ok()?;
    Some(i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
}

/// Sequence making ids from the same millisecond unique.
static ID_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Derives a 128-bit item id from time, process id, sequence, content hash and path.
#[allow(dead_code)] // Used by `Item::new`, which Task 8 wires up.
fn new_id(path: &Path, hash: u64) -> String {
    let sequence = ID_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let mut hasher = blake3::Hasher::new();
    hasher.update(&now_ms().to_le_bytes());
    hasher.update(&std::process::id().to_le_bytes());
    hasher.update(&sequence.to_le_bytes());
    hasher.update(&hash.to_le_bytes());
    hasher.update(path.as_os_str().to_string_lossy().as_bytes());
    hasher.finalize().to_hex()[..ITEM_ID_HEX_LEN].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    use serde_json::json;

    /// Scratch directory under `target/` (git-ignored); never touches `%LOCALAPPDATA%`.
    fn test_dir(tag: &str) -> PathBuf {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("item-tests")
            .join(format!("{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create the item test dir");
        dir
    }

    #[test]
    fn new_captures_file_metadata_and_an_opaque_unique_id() {
        let dir = test_dir("new");
        let path = dir.join("shot.png");
        fs::write(&path, b"not really a png").expect("create the test file");

        let item = Item::new(
            path.clone(),
            ItemKind::SystemScreenshot,
            0xDEAD_BEEF_CAFE_BABE,
            640,
            480,
        );
        assert_eq!(item.path, path, "the path must be stored verbatim");
        assert_eq!(item.kind, ItemKind::SystemScreenshot);
        assert_eq!(item.hash, 0xDEAD_BEEF_CAFE_BABE);
        assert_eq!((item.width, item.height), (640, 480));
        assert_eq!(item.id.len(), ITEM_ID_HEX_LEN);
        assert!(
            item.id.chars().all(|c| c.is_ascii_hexdigit()),
            "ids are lowercase hex: {}",
            item.id
        );
        assert!(item.created_at > 0, "file creation time must be recorded");
        assert!(
            item.added_at >= item.created_at,
            "an item cannot be hung before its file was created"
        );
        assert_eq!(
            item.updated_at, item.added_at,
            "a new item is not rewritten yet: updatedAt starts at addedAt"
        );
        assert!(item.note.is_empty(), "a new item has no note yet");
        assert!(item.tags.is_empty(), "a new item has no tags yet");

        // Two items for the same file must still be distinguishable.
        let second = Item::new(path, ItemKind::SystemScreenshot, 0xDEAD_BEEF_CAFE_BABE, 640, 480);
        assert_ne!(item.id, second.id);

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_file_still_yields_a_usable_item() {
        // Watchers compute metadata first, but a file can vanish between scan and creation; the
        // item must not panic and simply falls back to the current time.
        let dir = test_dir("missing");
        let path = dir.join("gone.png");
        let item = Item::new(path, ItemKind::WatchedFolder, 1, 0, 0);
        assert!(item.created_at > 0);
        assert!(item.added_at >= item.created_at);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn item_kinds_serialize_as_camel_case() {
        for (kind, expected) in [
            (ItemKind::Capture, "capture"),
            (ItemKind::SystemScreenshot, "systemScreenshot"),
            (ItemKind::Clipboard, "clipboard"),
        (ItemKind::Dropped, "dropped"),
            (ItemKind::Recording, "recording"),
            (ItemKind::WatchedFolder, "watchedFolder"),
        ] {
            assert_eq!(serde_json::to_value(kind).unwrap(), json!(expected));
        }
    }

    #[test]
    fn item_round_trips_with_camel_case_fields() {
        let item = Item {
            id: "0123456789abcdef0123456789abcdef".to_string(),
            path: PathBuf::from(r"C:\Users\Example\Pictures\Screenshots\shot.png"),
            thumb_path: None,
            kind: ItemKind::WatchedFolder,
            hash: 7,
            width: 10,
            height: 20,
            created_at: 1_700_000_000_000,
            added_at: 1_700_000_000_500,
            updated_at: 1_700_000_000_900,
            note: "A useful screenshot".to_string(),
            tags: vec!["work".to_string(), "reference".to_string()],
        };
        let value = serde_json::to_value(&item).expect("serialize item");
        assert_eq!(
            value,
            json!({
                "id": "0123456789abcdef0123456789abcdef",
                "path": r"C:\Users\Example\Pictures\Screenshots\shot.png",
                "kind": "watchedFolder",
                "hash": 7,
                "width": 10,
                "height": 20,
                "createdAt": 1_700_000_000_000i64,
                "addedAt": 1_700_000_000_500i64,
                "updatedAt": 1_700_000_000_900i64,
                "note": "A useful screenshot",
                "tags": ["work", "reference"],
            }),
            "a None thumbPath is omitted from state.json; note/tags are always written"
        );
        let parsed: Item = serde_json::from_value(value).expect("deserialize item");
        assert_eq!(parsed, item);
    }

    /// W2.1: `note`/`tags` survive a round-trip, and a pre-Wave-2 row (no such fields) loads with
    /// the serde defaults — the read-time migration the task requires.
    #[test]
    fn note_and_tags_round_trip_and_legacy_rows_default_to_empty() {
        let mut item = Item::new(
            PathBuf::from(r"C:\Users\Example\Pictures\Screenshots\shot.png"),
            ItemKind::Capture,
            7,
            800,
            600,
        );
        item.note = "remember this 备注".to_string();
        item.tags = vec!["截图".to_string(), "工作".to_string()];
        let value = serde_json::to_value(&item).expect("serialize item");
        assert_eq!(value.get("note").and_then(|note| note.as_str()), Some("remember this 备注"));
        assert_eq!(
            value.get("tags"),
            Some(&json!(["截图", "工作"])),
            "tags serialize as a JSON array in item order"
        );
        let parsed: Item = serde_json::from_value(value).expect("deserialize item");
        assert_eq!(parsed, item);

        // A state.json written before Wave 2 has neither field: both default to empty.
        let legacy = json!({
            "id": "0123456789abcdef0123456789abcdef",
            "path": r"C:\shot.png",
            "kind": "capture",
            "hash": 1,
            "width": 10,
            "height": 20,
            "createdAt": 1,
            "addedAt": 2,
            "updatedAt": 3,
        });
        let parsed: Item = serde_json::from_value(legacy).expect("legacy item");
        assert_eq!(parsed.note, "");
        assert!(parsed.tags.is_empty());
        assert_eq!(parsed.updated_at, 3, "migration must not rewrite other fields");
    }

    #[test]
    fn thumb_path_round_trips_and_defaults_to_none() {
        // P2-7: a persisted thumbPath survives a state.json round-trip.
        let mut item = Item::new(
            PathBuf::from(r"C:\Users\Example\Pictures\Screenshots\shot.png"),
            ItemKind::SystemScreenshot,
            7,
            4000,
            3000,
        );
        item.thumb_path = Some(PathBuf::from(
            r"C:\Users\Example\AppData\Local\Hangline\Thumbnails\0123.png",
        ));
        let value = serde_json::to_value(&item).expect("serialize item");
        assert_eq!(
            value.get("thumbPath").and_then(|thumb| thumb.as_str()),
            Some(r"C:\Users\Example\AppData\Local\Hangline\Thumbnails\0123.png")
        );
        let parsed: Item = serde_json::from_value(value).expect("deserialize item");
        assert_eq!(parsed.thumb_path, item.thumb_path);

        // A pre-P2-7 state file has no thumbPath: deserialization must default to None.
        let legacy = json!({
            "id": "0123456789abcdef0123456789abcdef",
            "path": r"C:\shot.png",
            "kind": "capture",
            "hash": 1,
            "width": 10,
            "height": 20,
            "createdAt": 1,
            "addedAt": 2,
        });
        let parsed: Item = serde_json::from_value(legacy).expect("legacy item");
        assert_eq!(parsed.thumb_path, None);
        assert_eq!(
            parsed.updated_at, 0,
            "a pre-P3 state.json has no updatedAt: the migration keeps 0"
        );
    }

    #[test]
    fn updated_at_round_trips_and_old_states_default_to_zero() {
        // P3: markup write-back bumps updatedAt, and the frontend uses it as the asset cache key.
        let mut item = Item::new(
            PathBuf::from(r"C:\Users\Example\Pictures\Screenshots\shot.png"),
            ItemKind::Capture,
            7,
            800,
            600,
        );
        item.updated_at = 1_700_000_123_456;
        let value = serde_json::to_value(&item).expect("serialize item");
        assert_eq!(
            value.get("updatedAt").and_then(|updated| updated.as_i64()),
            Some(1_700_000_123_456)
        );
        let parsed: Item = serde_json::from_value(value).expect("deserialize item");
        assert_eq!(parsed, item);

        // A pre-P3 row has no updatedAt at all and must load with the serde default (0).
        let legacy = json!({
            "id": "0123456789abcdef0123456789abcdef",
            "path": r"C:\shot.png",
            "kind": "capture",
            "hash": 1,
            "width": 10,
            "height": 20,
            "createdAt": 1,
            "addedAt": 2,
        });
        let parsed: Item = serde_json::from_value(legacy).expect("legacy item");
        assert_eq!(parsed.updated_at, 0);
        assert_eq!(parsed.added_at, 2, "migration must not rewrite addedAt");
    }
}
