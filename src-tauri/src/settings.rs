//! Application settings: the design-spec 3.6 schema, validation for IPC writes, and atomic
//! persistence.
//!
//! # Schema
//!
//! The 18 design-spec keys are serialized in camelCase (`captureHotkey`, `keepOnLine`, ...) so the
//! JSON file and the IPC payloads share the same names as the TypeScript `Settings` interface; the
//! `keepForDays` age-retention extension follows the same rule, as do the Wave 1 additions
//! `captureCopyOnFinish`/`editLatestHotkey`, the Wave 2 `searchHotkey` and the v1.4 appearance
//! settings `ropeStyle`/`ropeColor`. Unknown keys
//! in an existing file are ignored and missing keys fall back to [`Settings::default`]
//! (`#[serde(default)]`), which is the migration path for files written by older builds.
//!
//! # Persistence
//!
//! `settings.json` is written through a `.tmp` file + rename (the rename is atomic on NTFS), with
//! a best-effort `fsync` before the rename and the previous good file rotated to
//! `settings.json.bak` (its own `.tmp` + rename, so the backup is never partial). The backup is the
//! second layer: `load` recovers a corrupt main file from it and writes the recovered settings back
//! to the main file. Only when there is no usable backup does the corrupt file get preserved as
//! `settings.json.corrupt` and replaced with defaults (the third layer); a missing file is seeded
//! with defaults. A main file that exists but cannot be read is never overwritten — the write is
//! refused with a diagnostic error and the session stays read-only for that document.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use thiserror::Error;

use crate::hotkeys;

/// Maximum number of extra watched folders (design spec 3.5: "添加文件夹…（上限 3）").
pub const MAX_EXTRA_FOLDERS: usize = 3;

/// Upper bound for `revealPeekMs`. The spec only fixes the default (2500 ms); one minute keeps a
/// hand-edited file from pinning the line open for an absurd amount of time.
pub const MAX_REVEAL_PEEK_MS: u32 = 60_000;

/// Maximum length of the `language` tag (long enough for BCP 47 tags such as `zh-Hans`).
pub const MAX_LANGUAGE_LEN: usize = 32;

/// File name of the settings document inside the Hangline data directory.
const SETTINGS_FILE: &str = "settings.json";

/// Card scale on the line (design spec 3.5 "尺寸").
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CardSize {
    /// Small cards.
    Small,
    /// Medium cards (the spec default).
    #[default]
    Medium,
    /// Large cards.
    Large,
}

/// Rope material/pattern (v1.4 appearance setting `ropeStyle`).
///
/// Serialized as the lowercase wire names `silk`/`linen`/`brass`/`aurora`; the frontend renders the
/// rope from the same enum, so an unknown value must be rejected rather than silently degraded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RopeStyle {
    /// Smooth silk ribbon (default).
    #[default]
    Silk,
    /// Woven linen cord.
    Linen,
    /// Brass chain.
    Brass,
    /// Aurora gradient ribbon.
    Aurora,
}

/// Rope colour (v1.4 appearance setting `ropeColor`).
///
/// Serialized as the lowercase wire names `ink`/`ivory`/`champagne`/`fog`/`sage`/`haze`/
/// `terracotta`/`violet`; the frontend maps each name to its palette, so the enum is the contract.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RopeColor {
    /// Ink black (default).
    #[default]
    Ink,
    /// Ivory white.
    Ivory,
    /// Champagne gold.
    Champagne,
    /// Fog grey.
    Fog,
    /// Sage green.
    Sage,
    /// Haze blue.
    Haze,
    /// Terracotta.
    Terracotta,
    /// Violet.
    Violet,
}

/// Colour theme (design spec 3.6 `theme`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    /// Follow the Windows app theme.
    #[default]
    System,
    /// Force the light theme.
    Light,
    /// Force the dark theme.
    Dark,
}

impl Theme {
    /// Wire name used in the capture/shortcut window URLs (`?theme=system|light|dark`, P2-8).
    pub fn as_str(self) -> &'static str {
        match self {
            Theme::System => "system",
            Theme::Light => "light",
            Theme::Dark => "dark",
        }
    }
}

/// How many cards stay on the line (design spec 3.6 `keepOnLine`).
///
/// [`KeepOnLine::Limit`] is a hard cap on the state: only the newest `n` cards stay, older cards
/// leave the line (their files stay on disk; see `app_state`'s retention docs). [`KeepOnLine::Fit`]
/// is a view policy only: the state is deliberately unbounded and the panel renders what fits.
///
/// On the wire this is the string `"fit"` or the numbers `25`/`50`/`100`; the manual
/// `Serialize`/`Deserialize` implementations below produce and accept exactly those forms.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeepOnLine {
    /// Keep as many cards as the line can fit (default).
    Fit,
    /// Keep at most this many cards; only 25/50/100 are valid.
    Limit(u16),
}

impl Default for KeepOnLine {
    fn default() -> Self {
        Self::Fit
    }
}

/// Helper for the two accepted JSON shapes of [`KeepOnLine`].
#[derive(Deserialize)]
#[serde(untagged)]
enum KeepOnLineRepr {
    Word(String),
    Count(u16),
}

impl Serialize for KeepOnLine {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Fit => serializer.serialize_str("fit"),
            Self::Limit(limit) => serializer.serialize_u16(*limit),
        }
    }
}

impl<'de> Deserialize<'de> for KeepOnLine {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match KeepOnLineRepr::deserialize(deserializer)? {
            KeepOnLineRepr::Word(word) if word == "fit" => Ok(Self::Fit),
            KeepOnLineRepr::Count(limit) if matches!(limit, 25 | 50 | 100) => Ok(Self::Limit(limit)),
            other => Err(serde::de::Error::custom(format!(
                "keepOnLine must be \"fit\" or one of 25, 50, 100; got {}",
                match other {
                    KeepOnLineRepr::Word(word) => format!("\"{word}\""),
                    KeepOnLineRepr::Count(limit) => limit.to_string(),
                }
            ))),
        }
    }
}

/// How long cards may stay on the line before the age sweep takes them down (`keepForDays`).
///
/// [`KeepForDays::Forever`] is the default and the pre-existing (1.0) behavior: cards only leave
/// the line through the keep-count cap, a manual take-down or a missing file. [`KeepForDays::Days`]
/// is a fixed 7/30/90-day window; it is an extension over the 18 design-spec settings, owned by the
/// tray "Keep for" submenu. Expiry is evaluated by `AppState::trim_expired` (state) and
/// `retention::sweep` (lifecycle + Recycle Bin cleanup).
///
/// On the wire this is the string `"forever"` or the numbers `7`/`30`/`90`; the manual
/// `Serialize`/`Deserialize` implementations below produce and accept exactly those forms.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeepForDays {
    /// Never expire cards (default).
    Forever,
    /// Drop cards older than this many days; only 7/30/90 are valid.
    Days(u16),
}

impl Default for KeepForDays {
    fn default() -> Self {
        Self::Forever
    }
}

/// Helper for the two accepted JSON shapes of [`KeepForDays`].
#[derive(Deserialize)]
#[serde(untagged)]
enum KeepForDaysRepr {
    Word(String),
    Count(u16),
}

impl Serialize for KeepForDays {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Forever => serializer.serialize_str("forever"),
            Self::Days(days) => serializer.serialize_u16(*days),
        }
    }
}

impl<'de> Deserialize<'de> for KeepForDays {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match KeepForDaysRepr::deserialize(deserializer)? {
            KeepForDaysRepr::Word(word) if word == "forever" => Ok(Self::Forever),
            KeepForDaysRepr::Count(days) if matches!(days, 7 | 30 | 90) => Ok(Self::Days(days)),
            other => Err(serde::de::Error::custom(format!(
                "keepForDays must be \"forever\" or one of 7, 30, 90; got {}",
                match other {
                    KeepForDaysRepr::Word(word) => format!("\"{word}\""),
                    KeepForDaysRepr::Count(days) => days.to_string(),
                }
            ))),
        }
    }
}

/// The 18 design-spec user settings plus the `keepForDays` age-retention extension, the Wave 1/2
/// additions (`captureCopyOnFinish`, `editLatestHotkey`, `searchHotkey`) and the v1.4 appearance
/// settings (`ropeStyle`, `ropeColor`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    /// Global capture accelerator.
    pub capture_hotkey: String,
    /// Global show/hide accelerator.
    pub toggle_hotkey: String,
    /// Global "edit the newest card" accelerator (Wave 1 W1.8).
    pub edit_latest_hotkey: String,
    /// Global "open search" accelerator (Wave 2 W2.3).
    pub search_hotkey: String,
    /// Copy a confirmed screenshot to the clipboard after saving it (Wave 1 W1.5, default off).
    pub capture_copy_on_finish: bool,
    /// Watch `%USERPROFILE%\Pictures\Screenshots`.
    pub watch_system_screenshots: bool,
    /// Watch image data copied to the clipboard.
    pub watch_clipboard: bool,
    /// Watch `%USERPROFILE%\Videos\Captures`.
    pub watch_recordings: bool,
    /// Extra watched folders (at most [`MAX_EXTRA_FOLDERS`]).
    pub extra_folders: Vec<String>,
    /// Reveal the line when a new card arrives.
    pub reveal_on_new: bool,
    /// How long the line stays revealed when it was shown by a peek.
    pub reveal_peek_ms: u32,
    /// Reveal the line when the pointer rests on the top edge.
    pub top_edge_reveal: bool,
    /// Keep the line visible above full-screen applications.
    pub show_over_full_screen: bool,
    /// Maximum number of cards to keep.
    pub keep_on_line: KeepOnLine,
    /// Age-retention policy (`"forever"` or 7/30/90 days; design extension).
    pub keep_for_days: KeepForDays,
    /// Card scale.
    pub card_size: CardSize,
    /// Rope material/style (v1.4 appearance; default silk).
    pub rope_style: RopeStyle,
    /// Rope colour (v1.4 appearance; default ink).
    pub rope_color: RopeColor,
    /// Play copy/discard sounds.
    pub sound: bool,
    /// Start Hangline at login.
    pub start_at_login: bool,
    /// UI language tag (`"system"` = follow the OS).
    pub language: String,
    /// Colour theme.
    pub theme: Theme,
    /// Check for updates once a day.
    pub auto_check_updates: bool,
    /// ISO 8601 timestamp of the last update check (`""` = never checked).
    pub last_update_check: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            capture_hotkey: hotkeys::DEFAULT_CAPTURE.to_string(),
            toggle_hotkey: hotkeys::DEFAULT_TOGGLE.to_string(),
            edit_latest_hotkey: hotkeys::DEFAULT_EDIT_LATEST.to_string(),
            search_hotkey: hotkeys::DEFAULT_SEARCH.to_string(),
            capture_copy_on_finish: false,
            watch_system_screenshots: true,
            watch_clipboard: true,
            watch_recordings: true,
            extra_folders: Vec::new(),
            reveal_on_new: true,
            reveal_peek_ms: 2500,
            top_edge_reveal: false,
            show_over_full_screen: false,
            keep_on_line: KeepOnLine::Fit,
            keep_for_days: KeepForDays::Forever,
            card_size: CardSize::Medium,
            rope_style: RopeStyle::Silk,
            rope_color: RopeColor::Ink,
            sound: true,
            start_at_login: false,
            language: "system".to_string(),
            theme: Theme::System,
            auto_check_updates: true,
            last_update_check: String::new(),
        }
    }
}

/// Why a `set_setting` write was rejected.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum SettingsError {
    /// The key is not one of the accepted camelCase keys.
    #[error("unknown setting key: {0}")]
    UnknownKey(String),
    /// The value has the wrong JSON type or is outside the accepted range.
    #[error("invalid value for setting {key}: {reason}")]
    InvalidValue {
        /// The rejected key.
        key: String,
        /// Human-readable reason (shown to the settings UI).
        reason: String,
    },
}

impl Settings {
    /// Validates and applies one `set_setting` write.
    ///
    /// The write is all-or-nothing: a rejected key or value leaves the settings untouched. Keys
    /// are the camelCase JSON names (`captureHotkey`), not the Rust field names.
    pub fn set_json(&mut self, key: &str, value: Value) -> Result<(), SettingsError> {
        match key {
            "captureHotkey" => self.capture_hotkey = normalized_hotkey(key, &value)?,
            "toggleHotkey" => self.toggle_hotkey = normalized_hotkey(key, &value)?,
            "editLatestHotkey" => self.edit_latest_hotkey = normalized_hotkey(key, &value)?,
            "searchHotkey" => self.search_hotkey = normalized_hotkey(key, &value)?,
            "captureCopyOnFinish" => self.capture_copy_on_finish = expected_bool(key, &value)?,
            "watchSystemScreenshots" => {
                self.watch_system_screenshots = expected_bool(key, &value)?;
            }
            "watchClipboard" => self.watch_clipboard = expected_bool(key, &value)?,
            "watchRecordings" => self.watch_recordings = expected_bool(key, &value)?,
            "extraFolders" => self.extra_folders = validated_folders(key, &value)?,
            "revealOnNew" => self.reveal_on_new = expected_bool(key, &value)?,
            "revealPeekMs" => {
                let millis = value
                    .as_u64()
                    .ok_or_else(|| invalid(key, "expected a non-negative integer"))?;
                let millis = u32::try_from(millis)
                    .ok()
                    .filter(|millis| *millis <= MAX_REVEAL_PEEK_MS)
                    .ok_or_else(|| {
                        invalid(key, format!("must be at most {MAX_REVEAL_PEEK_MS} ms"))
                    })?;
                self.reveal_peek_ms = millis;
            }
            "topEdgeReveal" => self.top_edge_reveal = expected_bool(key, &value)?,
            "showOverFullScreen" => self.show_over_full_screen = expected_bool(key, &value)?,
            "keepOnLine" => self.keep_on_line = expected_enum(key, &value)?,
            "keepForDays" => self.keep_for_days = expected_enum(key, &value)?,
            "cardSize" => self.card_size = expected_enum(key, &value)?,
            "ropeStyle" => self.rope_style = expected_enum(key, &value)?,
            "ropeColor" => self.rope_color = expected_enum(key, &value)?,
            "sound" => self.sound = expected_bool(key, &value)?,
            "startAtLogin" => self.start_at_login = expected_bool(key, &value)?,
            "language" => self.language = validated_language(key, &value)?,
            "theme" => self.theme = expected_enum(key, &value)?,
            "autoCheckUpdates" => self.auto_check_updates = expected_bool(key, &value)?,
            "lastUpdateCheck" => self.last_update_check = validated_timestamp(key, &value)?,
            _ => return Err(SettingsError::UnknownKey(key.to_string())),
        }
        Ok(())
    }
}

/// Returns whether changing `key` can change the panel window geometry (Task 14).
///
/// `cardSize` changes the panel height (189/210/246 dp); `keepOnLine` does not change the window
/// size today, but is included because the same `set_setting` path must re-apply the geometry after
/// every line-limit change (a future keep-dependent layout must not be able to miss it). The
/// command uses this pure helper so the geometry path is unit-testable without a Tauri runtime.
pub fn affects_panel_geometry(key: &str) -> bool {
    matches!(key, "cardSize" | "keepOnLine")
}

/// Normalizes a watched-folder path for duplicate checks (Windows comparison).
///
/// Components are collected (trailing separators and `.` collapse) and lowercased; the same rule
/// as the asset-scope idempotency key, so "add this folder twice with different spelling" cannot
/// produce two watchers.
fn folder_key(path: &Path) -> String {
    path.components()
        .collect::<PathBuf>()
        .to_string_lossy()
        .to_ascii_lowercase()
}

/// Returns the `extraFolders` list with `path` appended, rejecting duplicates and the cap.
///
/// Used by the tray's "Add Folder…" action before it calls `set_setting`: the folder picker itself
/// cannot know the current list. `set_json` re-validates the same rules, so this helper is only a
/// friendly pre-check (the error becomes a log line, the returned list the `set_setting` value).
pub fn folder_list_with(folders: &[String], path: &Path) -> Result<Vec<String>, String> {
    if folders.len() >= MAX_EXTRA_FOLDERS {
        return Err(format!(
            "at most {MAX_EXTRA_FOLDERS} extra folders can be watched"
        ));
    }
    if !path.is_absolute() {
        return Err(format!(
            "the chosen folder must be an absolute path: {}",
            path.display()
        ));
    }
    let text = path.to_string_lossy().trim().to_string();
    if text.is_empty() {
        return Err("the chosen folder path is empty".to_string());
    }
    let key = folder_key(Path::new(&text));
    if folders.iter().any(|folder| folder_key(Path::new(folder)) == key) {
        return Err(format!("{text} is already watched"));
    }
    let mut next = folders.to_vec();
    next.push(text);
    Ok(next)
}

/// Returns the `extraFolders` list without the entry at `index` (unknown index = unchanged).
pub fn folder_list_without(folders: &[String], index: usize) -> Vec<String> {
    folders
        .iter()
        .enumerate()
        .filter(|(position, _)| *position != index)
        .map(|(_, folder)| folder.clone())
        .collect()
}

/// Returns `%LOCALAPPDATA%\Hangline`, creating it when missing.
///
/// This is the app's own data directory (the same base as
/// [`crate::capture::screenshots_dir`]'s `Screenshots` subdirectory), deliberately independent of
/// Tauri's `app_local_data_dir()` (`%LOCALAPPDATA%\app.hangline.Hangline`) so `settings.json` and
/// the asset-protocol scope `$LOCALDATA/Hangline/**` agree. A missing `LOCALAPPDATA` is an error
/// rather than a silent fallback outside the scoped directory.
pub fn data_dir() -> anyhow::Result<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("LOCALAPPDATA is not set; cannot locate %LOCALAPPDATA%\\Hangline"))?;
    let dir = PathBuf::from(base).join("Hangline");
    fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create the Hangline data dir {}", dir.display()))?;
    Ok(dir)
}

/// Absolute path of the settings document inside `dir`.
pub fn settings_path(dir: &Path) -> PathBuf {
    dir.join(SETTINGS_FILE)
}

/// Reads `settings.json` with the three-layer fallback: main file, last good `.bak`, defaults.
///
/// A healthy file is returned untouched. A corrupt (readable but unparsable) file is tried against
/// `settings.json.bak`: a valid backup is restored *and* written back to the main file, so the next
/// read is healthy again; the corrupt file itself is kept as `settings.json.corrupt` for diagnosis.
/// With no usable backup the corrupt file is replaced with defaults (same `.corrupt` preservation).
/// A missing file is seeded with defaults (first run).
///
/// Read I/O errors other than "not found" never write: the session runs on in-memory defaults and
/// the unreadable file is left alone ([`write_atomic`] refuses to overwrite it as well).
pub fn load(dir: &Path) -> Settings {
    let path = settings_path(dir);
    match fs::read(&path) {
        Ok(bytes) => match serde_json::from_slice::<Settings>(&bytes) {
            Ok(settings) => {
                log::info!("settings: loaded {}", path.display());
                settings
            }
            Err(err) => {
                log::error!(
                    "settings: {} is not valid settings JSON ({err})",
                    path.display()
                );
                recover_settings(dir, &path)
            }
        },
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            log::info!(
                "settings: {} does not exist yet; writing defaults",
                path.display()
            );
            let settings = Settings::default();
            persist_defaults(dir, &settings);
            settings
        }
        Err(err) => {
            log::error!(
                "settings: failed to read {} ({err}); using in-memory defaults (the file is left \
                 untouched and later writes are refused)",
                path.display()
            );
            Settings::default()
        }
    }
}

/// Repairs a corrupt `settings.json` from the last good `.bak`, or falls back to defaults.
///
/// The corrupt main file is moved aside *before* the recovered settings are written, so the write
/// does not rotate the corrupt bytes over the good backup.
fn recover_settings(dir: &Path, path: &Path) -> Settings {
    if let Some(recovered) = read_backup::<Settings>(path) {
        backup_corrupt(path);
        if let Err(err) = save_atomic(dir, &recovered) {
            log::error!(
                "settings: failed to write the recovered settings back to {}: {err:#}",
                path.display()
            );
        }
        recovered
    } else {
        log::warn!("settings: no usable backup; using defaults");
        backup_corrupt(path);
        let settings = Settings::default();
        persist_defaults(dir, &settings);
        settings
    }
}

/// Writes `settings.json` atomically (`.tmp` + rename, best-effort `fsync`).
pub fn save_atomic(dir: &Path, settings: &Settings) -> anyhow::Result<()> {
    let bytes = serde_json::to_vec_pretty(settings).context("failed to serialize settings")?;
    write_atomic(&settings_path(dir), &bytes)
}

/// Logs (instead of failing startup) when defaults cannot be seeded.
fn persist_defaults(dir: &Path, settings: &Settings) {
    if let Err(err) = save_atomic(dir, settings) {
        log::error!("settings: failed to persist defaults: {err:#}");
    }
}

/// Moves a corrupt (readable but unparsable) file aside so the next read starts from a clean state.
///
/// Used for both `settings.json` and `state.json` when no usable `.bak` exists; the file is kept as
/// `<name>.corrupt` for diagnosis and the caller then writes defaults/empty data. Returns the backup
/// path on success.
pub(crate) fn backup_corrupt(path: &Path) -> Option<PathBuf> {
    let Some(name) = path.file_name() else {
        log::warn!("persistence: {} has no file name to back up", path.display());
        return None;
    };
    let backup = path.with_file_name(format!("{}.corrupt", name.to_string_lossy()));
    match fs::rename(path, &backup) {
        Ok(()) => {
            log::warn!(
                "persistence: kept the unreadable file as {}",
                backup.display()
            );
            Some(backup)
        }
        Err(err) => {
            log::warn!(
                "persistence: failed to back up {} as {}: {err}",
                path.display(),
                backup.display()
            );
            None
        }
    }
}

/// Returns the sibling path holding the previous good copy of a persisted document.
///
/// Used for both `settings.json.bak` and `state.json.bak`; `None` when `path` has no file name.
pub(crate) fn backup_path(path: &Path) -> Option<PathBuf> {
    let name = path.file_name()?;
    Some(path.with_file_name(format!("{}.bak", name.to_string_lossy())))
}

/// Reads the last good copy (`<name>.bak`) of a persisted document, if there is a usable one.
///
/// Returns `None` when the backup is missing, unreadable or corrupt: callers then fall back to
/// defaults/empty exactly as before. The backup is validated with the same parser as the main
/// document, because it is only *expected* to be a previous good file (a hand edit or a partial
/// copy must not be trusted blindly).
pub(crate) fn read_backup<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let backup = backup_path(path)?;
    let bytes = match fs::read(&backup) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return None,
        Err(err) => {
            log::warn!(
                "persistence: failed to read the backup {} ({err})",
                backup.display()
            );
            return None;
        }
    };
    match serde_json::from_slice::<T>(&bytes) {
        Ok(value) => {
            log::info!(
                "persistence: recovered {} from the previous good copy {}",
                path.display(),
                backup.display()
            );
            Some(value)
        }
        Err(err) => {
            log::warn!(
                "persistence: the backup {} is not valid either ({err})",
                backup.display()
            );
            None
        }
    }
}

/// Writes one JSON document atomically: `<name>.tmp` then rename over `<name>`.
///
/// Before the rename the current file is rotated into `<name>.bak` (using the bytes read for the
/// guard below, written through its own `.tmp` + rename so the backup is never partial). The
/// rotation is best effort — a save must not fail because the backup slot is unusable; the main
/// file is the authoritative copy.
///
/// A target that exists but cannot be read (any error but `NotFound`) is a hard stop: overwriting it
/// would permanently destroy a file this process could not even inspect, so the write is refused
/// with a diagnostic error and the file is left in place. A corrupt *readable* file keeps following
/// the normal backup-and-rebuild path.
///
/// The temporary file is removed when anything fails, so a `.tmp` never masquerades as data.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let Some(name) = path.file_name() else {
        bail!("path has no file name: {}", path.display());
    };
    // One read serves both the unreadable-target guard and the backup rotation, so the file that is
    // preserved is exactly the one whose readability was checked.
    let previous = match fs::read(path) {
        Ok(bytes) => Some(bytes),
        Err(err) if err.kind() == io::ErrorKind::NotFound => None,
        Err(err) => {
            return Err(anyhow!(
                "refusing to overwrite the unreadable file {}: it exists but cannot be read \
                 ({err}); move it away or fix its permissions to resume saving",
                path.display()
            ));
        }
    };
    let tmp = path.with_file_name(format!("{}.tmp", name.to_string_lossy()));

    let result = write_file_synced(&tmp, bytes)
        .and_then(|()| {
            if let Some(previous) = previous.as_deref() {
                rotate_backup(path, previous);
            }
            Ok(())
        })
        .and_then(|()| rename_file(&tmp, path));
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Writes `bytes` to a temporary file and flushes it before it is renamed into place.
fn write_file_synced(tmp: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let mut file =
        fs::File::create(tmp).with_context(|| format!("failed to create {}", tmp.display()))?;
    file.write_all(bytes)
        .with_context(|| format!("failed to write {}", tmp.display()))?;
    if let Err(err) = file.sync_all() {
        // Best effort: the rename below still publishes the content atomically; fsync only
        // protects against a power loss between the write and the rename.
        log::warn!("persistence: fsync of {} failed: {err}", tmp.display());
    }
    drop(file);
    Ok(())
}

/// Renames `from` over `to`, publishing the content with one atomic replace.
fn rename_file(from: &Path, to: &Path) -> anyhow::Result<()> {
    fs::rename(from, to)
        .with_context(|| format!("failed to rename {} to {}", from.display(), to.display()))
}

/// Rotates the previous content of `path` into `<name>.bak` (own `.tmp` + rename).
///
/// Best effort: a save must not fail just because the backup slot is unusable; a warning is enough
/// for diagnosis and the new main file is still written losslessly.
fn rotate_backup(path: &Path, previous: &[u8]) {
    let Some(backup) = backup_path(path) else {
        log::warn!("persistence: {} has no backup name", path.display());
        return;
    };
    let Some(name) = backup.file_name() else {
        return;
    };
    let tmp = backup.with_file_name(format!("{}.tmp", name.to_string_lossy()));
    let result = write_file_synced(&tmp, previous).and_then(|()| rename_file(&tmp, &backup));
    if let Err(err) = result {
        let _ = fs::remove_file(&tmp);
        log::warn!(
            "persistence: failed to keep the previous {} as {}: {err:#}",
            path.display(),
            backup.display()
        );
    }
}

/// Builds the error for a rejected value.
fn invalid(key: &str, reason: impl Into<String>) -> SettingsError {
    SettingsError::InvalidValue {
        key: key.to_string(),
        reason: reason.into(),
    }
}

/// Requires a JSON boolean.
fn expected_bool(key: &str, value: &Value) -> Result<bool, SettingsError> {
    value
        .as_bool()
        .ok_or_else(|| invalid(key, "expected a boolean"))
}

/// Requires a JSON string and returns it.
fn expected_str<'v>(key: &str, value: &'v Value) -> Result<&'v str, SettingsError> {
    value
        .as_str()
        .ok_or_else(|| invalid(key, "expected a string"))
}

/// Deserializes a string enum (`CardSize`, `Theme`, `KeepOnLine`, `KeepForDays`), rejecting
/// unknown values.
fn expected_enum<T: DeserializeOwned>(key: &str, value: &Value) -> Result<T, SettingsError> {
    serde_json::from_value(value.clone()).map_err(|err| invalid(key, err.to_string()))
}

/// Normalizes a hotkey through the same parser used for registration.
fn normalized_hotkey(key: &str, value: &Value) -> Result<String, SettingsError> {
    let raw = expected_str(key, value)?;
    hotkeys::normalize(raw).map_err(|err| invalid(key, err.to_string()))
}

/// Validates the `extraFolders` list: at most 3 unique absolute paths.
fn validated_folders(key: &str, value: &Value) -> Result<Vec<String>, SettingsError> {
    let list = value
        .as_array()
        .ok_or_else(|| invalid(key, "expected an array of folder paths"))?;
    if list.len() > MAX_EXTRA_FOLDERS {
        return Err(invalid(
            key,
            format!("at most {MAX_EXTRA_FOLDERS} folders are supported"),
        ));
    }
    let mut folders: Vec<String> = Vec::with_capacity(list.len());
    for (index, entry) in list.iter().enumerate() {
        let text = entry
            .as_str()
            .ok_or_else(|| invalid(key, format!("entry {index} must be a string")))?;
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Err(invalid(key, format!("entry {index} is empty")));
        }
        if !Path::new(trimmed).is_absolute() {
            return Err(invalid(
                key,
                format!("entry {index} must be an absolute path: {trimmed}"),
            ));
        }
        if folders
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(trimmed))
        {
            return Err(invalid(key, format!("duplicate folder: {trimmed}")));
        }
        folders.push(trimmed.to_string());
    }
    Ok(folders)
}

/// Validates the `language` tag: a non-empty short ASCII/Unicode tag (`"system"` or BCP 47).
fn validated_language(key: &str, value: &Value) -> Result<String, SettingsError> {
    let language = expected_str(key, value)?.trim();
    if language.is_empty() {
        return Err(invalid(key, "must not be empty"));
    }
    if language.chars().count() > MAX_LANGUAGE_LEN {
        return Err(invalid(
            key,
            format!("must be at most {MAX_LANGUAGE_LEN} characters"),
        ));
    }
    if language.chars().any(char::is_control) {
        return Err(invalid(key, "must not contain control characters"));
    }
    Ok(language.to_string())
}

/// Validates `lastUpdateCheck`: `""` (never) or an ISO 8601 timestamp.
fn validated_timestamp(key: &str, value: &Value) -> Result<String, SettingsError> {
    let timestamp = expected_str(key, value)?;
    if !is_iso8601(timestamp) {
        return Err(invalid(
            key,
            "must be empty or an ISO 8601 timestamp (YYYY-MM-DDTHH:MM:SSZ)",
        ));
    }
    Ok(timestamp.to_string())
}

/// Shape check for the timestamps Hangline itself writes.
///
/// `YYYY-MM-DD[T ]HH:MM:SS` plus an optional fraction and `Z`/`±HH:MM` offset. Calendar fields
/// are range-checked; this is deliberately not a full ISO 8601 parser (no new dependency).
fn is_iso8601(value: &str) -> bool {
    if value.is_empty() {
        return true;
    }
    let bytes = value.as_bytes();
    if !value.is_ascii() || !(19..=40).contains(&bytes.len()) {
        return false;
    }
    let digits = |from: usize, to: usize| {
        bytes
            .get(from..to)
            .is_some_and(|slice| slice.iter().all(u8::is_ascii_digit))
    };
    let at = |index: usize, byte: u8| bytes.get(index) == Some(&byte);
    let number = |from: usize| {
        std::str::from_utf8(&bytes[from..from + 2])
            .ok()
            .and_then(|digits| digits.parse::<u32>().ok())
    };

    let shaped = digits(0, 4)
        && at(4, b'-')
        && digits(5, 7)
        && at(7, b'-')
        && digits(8, 10)
        && (at(10, b'T') || at(10, b't') || at(10, b' '))
        && digits(11, 13)
        && at(13, b':')
        && digits(14, 16)
        && at(16, b':')
        && digits(17, 19);
    if !shaped {
        return false;
    }

    let month = number(5).unwrap_or(0);
    let day = number(8).unwrap_or(0);
    let hour = number(11).unwrap_or(99);
    let minute = number(14).unwrap_or(99);
    let second = number(17).unwrap_or(99);
    (1..=12).contains(&month)
        && (1..=31).contains(&day)
        && hour <= 23
        && minute <= 59
        && second <= 59
        && is_iso8601_suffix(&value[19..])
}

/// Accepts the optional fractional-seconds + UTC/offset suffix of an ISO 8601 timestamp.
fn is_iso8601_suffix(suffix: &str) -> bool {
    let rest = if let Some(fraction) = suffix.strip_prefix('.') {
        let digits = fraction.len() - fraction.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        if digits == 0 || digits > 9 {
            return false;
        }
        &fraction[digits..]
    } else {
        suffix
    };
    rest.is_empty() || rest.eq_ignore_ascii_case("Z") || is_zone_offset(rest)
}

/// `±HH:MM` UTC offset.
fn is_zone_offset(rest: &str) -> bool {
    let bytes = rest.as_bytes();
    bytes.len() == 6
        && matches!(bytes[0], b'+' | b'-')
        && bytes[1..3].iter().all(u8::is_ascii_digit)
        && bytes[3] == b':'
        && bytes[4..6].iter().all(u8::is_ascii_digit)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;
    use std::path::PathBuf;

    use serde_json::json;

    /// Scratch directory under `target/` (git-ignored); never touches `%LOCALAPPDATA%`.
    fn test_dir(tag: &str) -> PathBuf {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("settings-tests")
            .join(format!("{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create the settings test dir");
        dir
    }

    #[test]
    fn defaults_match_spec() {
        let s = Settings::default();
        // All 18 design-spec keys (spec 3.6) plus the age-retention extension and Wave 1/2
        // additions, in table order.
        assert_eq!(s.capture_hotkey, "Ctrl+Alt+S");
        assert_eq!(s.toggle_hotkey, "Ctrl+Alt+T");
        assert_eq!(s.edit_latest_hotkey, "Ctrl+Alt+E");
        assert_eq!(s.search_hotkey, "Ctrl+Alt+F");
        assert!(
            !s.capture_copy_on_finish,
            "W1.5 must not change the existing save-only behaviour by default"
        );
        assert!(s.watch_system_screenshots);
        assert!(s.watch_clipboard);
        assert!(s.watch_recordings);
        assert!(s.extra_folders.is_empty());
        assert!(s.reveal_on_new);
        assert_eq!(s.reveal_peek_ms, 2500);
        assert!(!s.top_edge_reveal);
        assert!(!s.show_over_full_screen);
        assert_eq!(s.keep_on_line, KeepOnLine::Fit);
        assert_eq!(s.keep_for_days, KeepForDays::Forever);
        assert_eq!(s.card_size, CardSize::Medium);
        assert_eq!(s.rope_style, RopeStyle::Silk, "v1.4 default rope style");
        assert_eq!(s.rope_color, RopeColor::Ink, "v1.4 default rope colour");
        assert!(s.sound);
        assert!(!s.start_at_login);
        assert_eq!(s.language, "system");
        assert_eq!(s.theme, Theme::System);
        assert!(s.auto_check_updates);
        assert_eq!(s.last_update_check, "");
    }

    #[test]
    fn default_json_has_exactly_the_twenty_four_camel_case_keys() {
        let value = serde_json::to_value(Settings::default()).expect("serialize defaults");
        let object = value.as_object().expect("settings must be a JSON object");
        assert_eq!(
            object.len(),
            24,
            "18 design-spec keys + keepForDays + captureCopyOnFinish + editLatestHotkey + \
             searchHotkey + ropeStyle + ropeColor"
        );
        for key in [
            "captureHotkey",
            "toggleHotkey",
            "editLatestHotkey",
            "searchHotkey",
            "captureCopyOnFinish",
            "watchSystemScreenshots",
            "watchClipboard",
            "watchRecordings",
            "extraFolders",
            "revealOnNew",
            "revealPeekMs",
            "topEdgeReveal",
            "showOverFullScreen",
            "keepOnLine",
            "keepForDays",
            "cardSize",
            "ropeStyle",
            "ropeColor",
            "sound",
            "startAtLogin",
            "language",
            "theme",
            "autoCheckUpdates",
            "lastUpdateCheck",
        ] {
            assert!(object.contains_key(key), "missing camelCase key {key}");
        }
        assert_eq!(object["captureHotkey"], json!("Ctrl+Alt+S"));
        assert_eq!(object["editLatestHotkey"], json!("Ctrl+Alt+E"));
        assert_eq!(object["searchHotkey"], json!("Ctrl+Alt+F"));
        assert_eq!(object["captureCopyOnFinish"], json!(false));
        assert_eq!(object["keepOnLine"], json!("fit"));
        assert_eq!(object["keepForDays"], json!("forever"));
        assert_eq!(object["cardSize"], json!("medium"));
        assert_eq!(object["ropeStyle"], json!("silk"));
        assert_eq!(object["ropeColor"], json!("ink"));
        assert_eq!(object["theme"], json!("system"));
        assert_eq!(object["revealPeekMs"], json!(2500));
        assert_eq!(object["extraFolders"], json!([]));
    }

    #[test]
    fn unknown_key_is_rejected() {
        let mut s = Settings::default();
        assert!(s.set_json("nope", json!(1)).is_err());
        // Only the canonical camelCase keys are accepted (serde names, not Rust names).
        assert!(s.set_json("capture_hotkey", json!("Ctrl+Alt+S")).is_err());
        assert_eq!(
            s,
            Settings::default(),
            "a rejected key must not mutate settings"
        );
    }

    #[test]
    fn hotkeys_are_normalized_and_invalid_ones_rejected() {
        let mut s = Settings::default();
        s.set_json("captureHotkey", json!("ctrl+alt+shift+f5"))
            .expect("valid accelerator");
        assert_eq!(s.capture_hotkey, "Ctrl+Alt+Shift+F5");
        // W1.8: the third shortcut shares the same grammar/validation as the first two.
        s.set_json("editLatestHotkey", json!("shift+ctrl+e"))
            .expect("valid third accelerator");
        assert_eq!(s.edit_latest_hotkey, "Ctrl+Shift+E");
        s.set_json("editLatestHotkey", json!("Ctrl+Alt+E"))
            .expect("the spec default is a valid accelerator");
        assert_eq!(s.edit_latest_hotkey, "Ctrl+Alt+E");
        // W2.3: the fourth shortcut shares the same grammar/validation as the first three.
        s.set_json("searchHotkey", json!("alt+ctrl+f"))
            .expect("valid fourth accelerator");
        assert_eq!(s.search_hotkey, "Ctrl+Alt+F");
        s.set_json("searchHotkey", json!("Ctrl+Shift+F"))
            .expect("a changed search accelerator");
        assert_eq!(s.search_hotkey, "Ctrl+Shift+F");

        let before = s.clone();
        assert!(s.set_json("toggleHotkey", json!("T")).is_err());
        assert!(s.set_json("toggleHotkey", json!(42)).is_err());
        assert!(s.set_json("editLatestHotkey", json!("E")).is_err());
        assert!(s.set_json("editLatestHotkey", json!(null)).is_err());
        assert!(s.set_json("searchHotkey", json!("F")).is_err());
        assert!(s.set_json("searchHotkey", json!(7)).is_err());
        assert_eq!(
            s, before,
            "an invalid hotkey must not mutate settings"
        );
    }

    #[test]
    fn invalid_bool_and_enum_values_are_rejected() {
        let mut s = Settings::default();
        let before = s.clone();
        for (key, value) in [
            ("watchClipboard", json!(1)),
            ("watchClipboard", json!("true")),
            ("captureCopyOnFinish", json!(1)),
            ("captureCopyOnFinish", json!("true")),
            ("revealOnNew", json!(null)),
            ("cardSize", json!("huge")),
            ("cardSize", json!(2)),
            ("ropeStyle", json!("gold")),
            ("ropeStyle", json!(1)),
            ("ropeColor", json!("red")),
            ("ropeColor", json!(null)),
            ("theme", json!("sepia")),
            ("keepOnLine", json!(7)),
            ("keepOnLine", json!("25")),
            ("keepOnLine", json!(null)),
            ("keepForDays", json!(0)),
            ("keepForDays", json!(1)),
            ("keepForDays", json!("7")),
            ("keepForDays", json!("always")),
            ("keepForDays", json!(null)),
        ] {
            assert!(
                s.set_json(key, value.clone()).is_err(),
                "{key}={value} must be rejected"
            );
            assert_eq!(s, before, "{key} rejection must not mutate settings");
        }

        s.set_json("cardSize", json!("large")).unwrap();
        assert_eq!(s.card_size, CardSize::Large);
        s.set_json("ropeStyle", json!("aurora")).unwrap();
        assert_eq!(s.rope_style, RopeStyle::Aurora);
        s.set_json("ropeColor", json!("violet")).unwrap();
        assert_eq!(s.rope_color, RopeColor::Violet);
        s.set_json("theme", json!("dark")).unwrap();
        assert_eq!(s.theme, Theme::Dark);
        s.set_json("keepOnLine", json!(50)).unwrap();
        assert_eq!(s.keep_on_line, KeepOnLine::Limit(50));
        s.set_json("keepOnLine", json!("fit")).unwrap();
        assert_eq!(s.keep_on_line, KeepOnLine::Fit);
        s.set_json("keepForDays", json!(30)).unwrap();
        assert_eq!(s.keep_for_days, KeepForDays::Days(30));
        s.set_json("keepForDays", json!("forever")).unwrap();
        assert_eq!(s.keep_for_days, KeepForDays::Forever);
        s.set_json("watchClipboard", json!(false)).unwrap();
        assert!(!s.watch_clipboard);
    }

    #[test]
    fn extra_folders_are_limited_to_three_absolute_unique_paths() {
        let mut s = Settings::default();
        s.set_json("extraFolders", json!(["C:\\One", "D:\\Two", "C:\\Three"]))
            .expect("three absolute folders are allowed");
        assert_eq!(s.extra_folders.len(), 3);

        let before = s.clone();
        for value in [
            json!(["C:\\1", "C:\\2", "C:\\3", "C:\\4"]),
            json!(["relative\\dir"]),
            json!([""]),
            json!([42]),
            json!(["C:\\One", "c:\\one"]),
            json!("C:\\One"),
        ] {
            assert!(
                s.set_json("extraFolders", value.clone()).is_err(),
                "{value} must be rejected"
            );
            assert_eq!(s, before, "rejected folders must not mutate settings");
        }

        s.set_json("extraFolders", json!([])).unwrap();
        assert!(s.extra_folders.is_empty());
    }

    #[test]
    fn numeric_and_text_ranges_are_validated() {
        let mut s = Settings::default();
        s.set_json("revealPeekMs", json!(0)).expect("0 ms is allowed");
        assert_eq!(s.reveal_peek_ms, 0);
        s.set_json("revealPeekMs", json!(60_000))
            .expect("60 s is allowed");
        assert_eq!(s.reveal_peek_ms, 60_000);
        let before = s.clone();
        for value in [
            json!(-1),
            json!(60_001),
            json!(2500.5),
            json!("2500"),
            json!(u64::MAX),
        ] {
            assert!(
                s.set_json("revealPeekMs", value.clone()).is_err(),
                "{value} must be rejected"
            );
            assert_eq!(s, before);
        }

        s.set_json("language", json!("en")).unwrap();
        assert_eq!(s.language, "en");
        assert!(s.set_json("language", json!("")).is_err());
        assert!(s.set_json("language", json!(7)).is_err());
        assert!(s.set_json("language", json!("x".repeat(33))).is_err());
    }

    #[test]
    fn last_update_check_accepts_empty_or_iso8601_only() {
        let mut s = Settings::default();
        for accepted in [
            "",
            "2026-10-29T13:00:00Z",
            "2026-10-29T13:00:00.123Z",
            "2026-10-29T13:00:00+08:00",
            "2026-10-29 13:00:00",
        ] {
            s.set_json("lastUpdateCheck", json!(accepted))
                .unwrap_or_else(|err| panic!("{accepted:?} must be accepted: {err}"));
            assert_eq!(s.last_update_check, accepted);
        }
        let before = s.clone();
        for rejected in [
            json!("yesterday"),
            json!("2026-13-99"),
            json!("2026-10-29"),
            json!(1_700_000_000),
        ] {
            assert!(
                s.set_json("lastUpdateCheck", rejected.clone()).is_err(),
                "{rejected} must be rejected"
            );
            assert_eq!(s, before);
        }
    }

    #[test]
    fn save_and_load_round_trip_atomically() {
        let dir = test_dir("round-trip");
        let mut settings = Settings::default();
        settings.watch_clipboard = false;
        settings.extra_folders = vec!["C:\\Shots".to_string()];
        settings.keep_on_line = KeepOnLine::Limit(100);
        settings.capture_copy_on_finish = true;
        settings.edit_latest_hotkey = "Ctrl+Shift+E".to_string();
        settings.search_hotkey = "Ctrl+Shift+F".to_string();
        settings.rope_style = RopeStyle::Aurora;
        settings.rope_color = RopeColor::Terracotta;
        save_atomic(&dir, &settings).expect("save settings");

        let path = dir.join("settings.json");
        assert!(path.is_file(), "settings.json must exist");
        assert!(
            !dir.join("settings.json.tmp").exists(),
            "the .tmp file must be renamed away"
        );
        let json: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).expect("read settings")).expect("valid JSON");
        assert_eq!(json["watchClipboard"], json!(false));
        assert_eq!(json["extraFolders"], json!(["C:\\Shots"]));
        assert_eq!(json["captureCopyOnFinish"], json!(true));
        assert_eq!(json["editLatestHotkey"], json!("Ctrl+Shift+E"));
        assert_eq!(json["searchHotkey"], json!("Ctrl+Shift+F"));
        assert_eq!(json["ropeStyle"], json!("aurora"));
        assert_eq!(json["ropeColor"], json!("terracotta"));

        assert_eq!(load(&dir), settings);

        // A second save must atomically replace the previous file (rename over an existing file).
        settings.sound = false;
        settings.card_size = CardSize::Small;
        save_atomic(&dir, &settings).expect("replace settings");
        assert_eq!(load(&dir), settings);

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn load_missing_file_returns_and_seeds_defaults() {
        let dir = test_dir("missing");
        let loaded = load(&dir);
        assert_eq!(loaded, Settings::default());
        let path = dir.join("settings.json");
        assert!(path.is_file(), "the first run must seed settings.json");
        assert_eq!(
            serde_json::from_slice::<Settings>(&fs::read(&path).unwrap()).unwrap(),
            Settings::default()
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn load_corrupt_file_falls_back_to_defaults_and_keeps_a_backup() {
        let dir = test_dir("corrupt");
        let path = dir.join("settings.json");
        let corrupt = b"{ this is not json";
        fs::write(&path, corrupt).expect("write corrupt settings");

        assert_eq!(load(&dir), Settings::default());
        assert_eq!(
            fs::read(dir.join("settings.json.corrupt")).expect("corrupt file backup"),
            corrupt,
            "the unreadable file must be preserved for diagnosis"
        );
        assert_eq!(
            serde_json::from_slice::<Settings>(&fs::read(&path).unwrap()).unwrap(),
            Settings::default(),
            "a valid defaults file must replace the corrupt one"
        );
        let _ = fs::remove_dir_all(dir);
    }

    /// Three-layer persistence, layer 1: every successful save rotates the previous settings file
    /// into `settings.json.bak` before publishing the new one, so the last good generation survives
    /// a corrupt main file. Only one generation is kept, and no `.tmp` is left behind.
    #[test]
    fn save_rotates_the_previous_settings_into_bak() {
        let dir = test_dir("rotate-bak");
        let main = dir.join("settings.json");
        let backup = dir.join("settings.json.bak");
        let mut first = Settings::default();
        first.watch_clipboard = false;
        first.keep_on_line = KeepOnLine::Limit(25);
        save_atomic(&dir, &first).expect("first save");
        assert!(main.is_file());
        assert!(
            !backup.exists(),
            "the first save has no previous file to rotate"
        );

        let mut second = first.clone();
        second.watch_clipboard = true;
        second.card_size = CardSize::Large;
        save_atomic(&dir, &second).expect("second save");
        assert_eq!(
            serde_json::from_slice::<Settings>(&fs::read(&backup).expect("read the bak"))
                .expect("the backup is a complete settings document"),
            first,
            "the previous good settings must be rotated into settings.json.bak"
        );
        assert_eq!(load(&dir), second);
        assert!(!dir.join("settings.json.bak.tmp").exists());

        // Only one generation is kept: the third save replaces the backup with the second file.
        let mut third = second.clone();
        third.sound = false;
        save_atomic(&dir, &third).expect("third save");
        assert_eq!(
            serde_json::from_slice::<Settings>(&fs::read(&backup).unwrap()).unwrap(),
            second
        );
        assert_eq!(load(&dir), third);
        let _ = fs::remove_dir_all(dir);
    }

    /// Three-layer persistence, layer 2: a corrupt main file is recovered from the last good
    /// `.bak`, kept as `settings.json.corrupt` for diagnosis, and written back so the next start
    /// reads a healthy file again.
    #[test]
    fn load_recovers_a_corrupt_settings_file_from_the_backup() {
        let dir = test_dir("recover-bak");
        let mut good = Settings::default();
        good.capture_hotkey = "Ctrl+Shift+F9".to_string();
        good.keep_for_days = KeepForDays::Days(30);
        save_atomic(&dir, &good).expect("first save");
        let mut next = good.clone();
        next.watch_clipboard = false;
        save_atomic(&dir, &next).expect("second save"); // settings.json.bak == good

        let corrupt = b"{ this is still not json".to_vec();
        fs::write(dir.join("settings.json"), &corrupt).expect("corrupt the main file");

        assert_eq!(load(&dir), good, "the last good copy must be recovered");
        assert_eq!(
            fs::read(dir.join("settings.json.corrupt")).expect("corrupt file backup"),
            corrupt,
            "the corrupt file must still be preserved for diagnosis"
        );
        assert_eq!(
            serde_json::from_slice::<Settings>(&fs::read(dir.join("settings.json")).unwrap())
                .expect("the recovered settings must be written back as valid JSON"),
            good
        );
        assert_eq!(
            serde_json::from_slice::<Settings>(&fs::read(dir.join("settings.json.bak")).unwrap())
                .expect("the good backup stays readable"),
            good,
            "the corrupt main file must be moved aside before the write-back, so the good .bak survives"
        );
        assert_eq!(
            load(&dir),
            good,
            "the second load must read a healthy file (no recovery needed)"
        );
        let _ = fs::remove_dir_all(dir);
    }

    /// Three-layer persistence, layer 3: a corrupt main file whose `.bak` is also unusable falls
    /// back to defaults exactly like before, keeping the corrupt bytes as `settings.json.corrupt`.
    #[test]
    fn load_falls_back_to_defaults_when_the_settings_backup_is_unusable() {
        let dir = test_dir("bad-bak");
        let path = dir.join("settings.json");
        let corrupt = b"{ this is not json".to_vec();
        fs::write(&path, &corrupt).expect("write corrupt settings");
        fs::write(dir.join("settings.json.bak"), b"also not json").expect("write corrupt backup");

        assert_eq!(load(&dir), Settings::default());
        assert_eq!(
            fs::read(dir.join("settings.json.corrupt")).expect("corrupt file backup"),
            corrupt
        );
        assert_eq!(
            fs::read(dir.join("settings.json.bak")).expect("the unusable backup is left alone"),
            b"also not json",
            "a bad backup is not deleted; the next save rotates a good generation over it"
        );
        assert_eq!(
            serde_json::from_slice::<Settings>(&fs::read(&path).unwrap()).unwrap(),
            Settings::default(),
            "defaults must replace the corrupt file"
        );
        let _ = fs::remove_dir_all(dir);
    }

    /// Three-layer persistence, read-only session rule: a target that exists but cannot be read
    /// must never be overwritten, because it could hold recoverable user data.
    ///
    /// Windows cannot make a plain file unreadable without ACLs (and this test sandbox may not
    /// allow changing them), so a *directory* is placed at the settings path: `fs::read` then fails
    /// with a non-`NotFound` I/O error on every supported platform (`PermissionDenied` on Windows,
    /// `IsADirectory`/`Other` on Unix) — exactly the "exists but unreadable" condition.
    #[test]
    fn save_refuses_to_overwrite_a_settings_file_it_cannot_read() {
        let dir = test_dir("read-failure");
        let path = dir.join("settings.json");
        fs::create_dir(&path).expect("a directory at the settings path blocks reads");

        let error = save_atomic(&dir, &Settings::default()).expect_err("the write must be refused");
        let message = format!("{error:#}");
        assert!(
            message.contains("refusing to overwrite"),
            "the error must be diagnostic: {message}"
        );
        assert!(
            message.contains("settings.json"),
            "the error names the path: {message}"
        );
        assert!(path.is_dir(), "the unreadable target must be left in place");
        assert!(
            !dir.join("settings.json.tmp").exists(),
            "a refused write must not leave a .tmp file"
        );
        assert!(
            !dir.join("settings.json.bak").exists(),
            "a refused write must not rotate a backup"
        );

        // The read side is a read-only session too: in-memory defaults, no write at all.
        assert_eq!(load(&dir), Settings::default());
        assert!(path.is_dir(), "load must not touch the unreadable file");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn load_partial_and_future_files_merge_with_defaults() {
        let dir = test_dir("partial");
        fs::write(
            dir.join("settings.json"),
            br#"{"watchClipboard": false, "keepOnLine": 50, "futureKey": true}"#,
        )
        .expect("write partial settings");

        let loaded = load(&dir);
        assert!(!loaded.watch_clipboard);
        assert_eq!(loaded.keep_on_line, KeepOnLine::Limit(50));
        // Everything else must keep the spec default (settings migration path), including the
        // Wave 1 additions: an old file must migrate to save-only with the "Ctrl+Alt+E" shortcut.
        assert_eq!(loaded.capture_hotkey, "Ctrl+Alt+S");
        assert_eq!(loaded.edit_latest_hotkey, "Ctrl+Alt+E");
        assert_eq!(
            loaded.search_hotkey, "Ctrl+Alt+F",
            "a settings file written before Wave 2 must migrate to the search default"
        );
        assert!(!loaded.capture_copy_on_finish);
        assert_eq!(
            loaded.rope_style,
            RopeStyle::Silk,
            "a settings file written before v1.4 must migrate to the default rope style"
        );
        assert_eq!(
            loaded.rope_color,
            RopeColor::Ink,
            "a settings file written before v1.4 must migrate to the default rope colour"
        );
        assert_eq!(loaded.reveal_peek_ms, 2500);
        assert!(loaded.reveal_on_new);
        assert_eq!(
            loaded.keep_for_days,
            KeepForDays::Forever,
            "a settings file written before keepForDays existed must keep cards forever"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn keeps_on_line_serializes_as_fit_or_limits() {
        assert_eq!(serde_json::to_value(KeepOnLine::Fit).unwrap(), json!("fit"));
        assert_eq!(
            serde_json::to_value(KeepOnLine::Limit(25)).unwrap(),
            json!(25)
        );
        for accepted in [json!("fit"), json!(25), json!(50), json!(100)] {
            let parsed: KeepOnLine = serde_json::from_value(accepted.clone()).unwrap();
            assert_eq!(serde_json::to_value(parsed).unwrap(), accepted);
        }
        for rejected in [json!(0), json!(7), json!("25"), json!("all"), json!(-25)] {
            assert!(
                serde_json::from_value::<KeepOnLine>(rejected.clone()).is_err(),
                "{rejected} must be rejected"
            );
        }
    }

    /// The age-retention policy serializes as `"forever"` or one of 7/30/90 (design extension);
    /// any other number/word would let a hand-edited settings.json bypass the fixed menu values.
    #[test]
    fn keep_for_days_serializes_as_forever_or_the_fixed_windows() {
        assert_eq!(
            serde_json::to_value(KeepForDays::Forever).unwrap(),
            json!("forever")
        );
        assert_eq!(
            serde_json::to_value(KeepForDays::Days(7)).unwrap(),
            json!(7)
        );
        for accepted in [json!("forever"), json!(7), json!(30), json!(90)] {
            let parsed: KeepForDays = serde_json::from_value(accepted.clone()).unwrap();
            assert_eq!(serde_json::to_value(parsed).unwrap(), accepted);
        }
        for rejected in [
            json!(0),
            json!(1),
            json!(60),
            json!("7"),
            json!("all"),
            json!(-7),
        ] {
            assert!(
                serde_json::from_value::<KeepForDays>(rejected.clone()).is_err(),
                "{rejected} must be rejected"
            );
        }
    }

    /// v1.4 appearance: both rope enums serialize as the documented lowercase wire names,
    /// round-trip through `set_json`/`Settings`, and unknown values are rejected without mutation.
    #[test]
    fn rope_appearance_serializes_lowercase_and_rejects_unknown_values() {
        for (style, wire) in [
            (RopeStyle::Silk, "silk"),
            (RopeStyle::Linen, "linen"),
            (RopeStyle::Brass, "brass"),
            (RopeStyle::Aurora, "aurora"),
        ] {
            assert_eq!(serde_json::to_value(style).expect("serialize style"), json!(wire));
            assert_eq!(
                serde_json::from_value::<RopeStyle>(json!(wire)).expect("parse style"),
                style
            );
        }
        for (color, wire) in [
            (RopeColor::Ink, "ink"),
            (RopeColor::Ivory, "ivory"),
            (RopeColor::Champagne, "champagne"),
            (RopeColor::Fog, "fog"),
            (RopeColor::Sage, "sage"),
            (RopeColor::Haze, "haze"),
            (RopeColor::Terracotta, "terracotta"),
            (RopeColor::Violet, "violet"),
        ] {
            assert_eq!(serde_json::to_value(color).expect("serialize color"), json!(wire));
            assert_eq!(
                serde_json::from_value::<RopeColor>(json!(wire)).expect("parse color"),
                color
            );
        }

        // `set_json` accepts every wire name and keeps the rest of the settings untouched.
        let mut settings = Settings::default();
        settings.set_json("ropeStyle", json!("brass")).expect("valid style");
        settings.set_json("ropeColor", json!("haze")).expect("valid color");
        assert_eq!(settings.rope_style, RopeStyle::Brass);
        assert_eq!(settings.rope_color, RopeColor::Haze);
        settings.set_json("ropeStyle", json!("silk")).expect("default style");
        settings.set_json("ropeColor", json!("ink")).expect("default color");
        assert_eq!(settings.rope_style, RopeStyle::Silk);
        assert_eq!(settings.rope_color, RopeColor::Ink);

        // A JSON round-trip through the whole settings document keeps both values.
        settings.set_json("ropeStyle", json!("linen")).expect("valid style");
        settings.set_json("ropeColor", json!("champagne")).expect("valid color");
        let value = serde_json::to_value(&settings).expect("serialize settings");
        assert_eq!(value["ropeStyle"], json!("linen"));
        assert_eq!(value["ropeColor"], json!("champagne"));
        let parsed: Settings = serde_json::from_value(value).expect("deserialize settings");
        assert_eq!(parsed, settings);

        // Unknown styles/colours are rejected with a diagnostic error and leave memory untouched.
        let before = settings.clone();
        for (key, value) in [
            ("ropeStyle", json!("gold")),
            ("ropeStyle", json!("Silk")),
            ("ropeStyle", json!(1)),
            ("ropeColor", json!("red")),
            ("ropeColor", json!("black")),
            ("ropeColor", json!(null)),
        ] {
            let error = settings
                .set_json(key, value.clone())
                .expect_err("must be rejected");
            assert!(
                matches!(error, SettingsError::InvalidValue { .. }),
                "expected InvalidValue for {key}={value}: {error}"
            );
            assert_eq!(settings, before, "{key} rejection must not mutate settings");
        }
    }

    /// Task 14: `set_setting` must re-apply the panel window geometry for the two keys that can
    /// change the card layout/size, and only for those (the check is a pure helper so the command
    /// path is unit-testable without a Tauri runtime).
    #[test]
    fn geometry_affecting_settings_are_card_size_and_keep_on_line() {
        assert!(affects_panel_geometry("cardSize"));
        assert!(affects_panel_geometry("keepOnLine"));
        for key in [
            "captureHotkey",
            "editLatestHotkey",
            "searchHotkey",
            "captureCopyOnFinish",
            "ropeStyle",
            "ropeColor",
            "theme",
            "watchClipboard",
            "language",
            "cardsize",
            "keeponline",
            "cardSize ",
            "",
        ] {
            assert!(
                !affects_panel_geometry(key),
                "{key:?} must not re-apply the panel geometry"
            );
        }
    }

    /// Task 15 folder picker: appending enforces the three-folder cap and Windows-style duplicate
    /// detection (case/trailing separator insensitive), removing drops exactly one entry.
    #[test]
    fn folder_list_helpers_enforce_cap_and_dedup() {
        let folders: Vec<String> = vec![r"C:\Shots".to_string()];
        assert_eq!(
            folder_list_with(&folders, Path::new(r"D:\More")).expect("second folder"),
            vec![r"C:\Shots".to_string(), r"D:\More".to_string()]
        );
        assert!(folder_list_with(&folders, Path::new(r"c:\shots\")).is_err());
        assert!(folder_list_with(&folders, Path::new(r"relative")).is_err());

        let full: Vec<String> = vec![
            r"C:\One".to_string(),
            r"C:\Two".to_string(),
            r"C:\Three".to_string(),
        ];
        assert!(folder_list_with(&full, Path::new(r"C:\Four")).is_err());
        assert_eq!(
            folder_list_without(&full, 1),
            vec![r"C:\One".to_string(), r"C:\Three".to_string()]
        );
        assert_eq!(folder_list_without(&full, 99), full);
    }

    #[test]
    fn data_dir_is_localappdata_hangline() {
        let dir = data_dir().expect("data dir");
        let local_app_data = std::env::var_os("LOCALAPPDATA").expect("LOCALAPPDATA is set");
        assert_eq!(dir, PathBuf::from(local_app_data).join("Hangline"));
        assert!(dir.is_dir(), "data_dir must create the directory");
    }

    #[test]
    fn theme_wire_names_match_serde_and_as_str() {
        // P2-8: window URLs and `fly-geometry` use `Theme::as_str`; it must stay identical to the
        // serde wire form so a renamed variant cannot silently degrade to `system` in a window.
        for (theme, wire) in [
            (Theme::System, "system"),
            (Theme::Light, "light"),
            (Theme::Dark, "dark"),
        ] {
            assert_eq!(theme.as_str(), wire);
            assert_eq!(serde_json::to_value(theme).expect("serialize theme"), json!(wire));
        }
    }
}
