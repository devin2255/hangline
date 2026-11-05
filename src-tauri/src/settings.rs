//! Application settings: the design-spec 3.6 schema, validation for IPC writes, and atomic
//! persistence.
//!
//! # Schema
//!
//! The 18 keys are serialized in camelCase (`captureHotkey`, `keepOnLine`, ...) so the JSON file
//! and the IPC payloads share the same names as the TypeScript `Settings` interface. Unknown keys
//! in an existing file are ignored and missing keys fall back to [`Settings::default`]
//! (`#[serde(default)]`), which is the migration path for files written by older builds.
//!
//! # Persistence
//!
//! `settings.json` is written through a `.tmp` file + rename (the rename is atomic on NTFS), with
//! a best-effort `fsync` before the rename. Writes only happen on change or on first run; reads
//! never rewrite a healthy file. A missing file is seeded with defaults, an unreadable file is
//! preserved as `settings.json.corrupt` and replaced with defaults.

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

/// The 18 user settings from design spec 3.6.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    /// Global capture accelerator.
    pub capture_hotkey: String,
    /// Global show/hide accelerator.
    pub toggle_hotkey: String,
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
    /// Card scale.
    pub card_size: CardSize,
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
            watch_system_screenshots: true,
            watch_clipboard: true,
            watch_recordings: true,
            extra_folders: Vec::new(),
            reveal_on_new: true,
            reveal_peek_ms: 2500,
            top_edge_reveal: false,
            show_over_full_screen: false,
            keep_on_line: KeepOnLine::Fit,
            card_size: CardSize::Medium,
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
    /// The key is not one of the 18 spec keys (camelCase only).
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
            "cardSize" => self.card_size = expected_enum(key, &value)?,
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

/// Reads `settings.json`, falling back to defaults on missing/corrupt files.
///
/// A missing file is seeded with the defaults (first run); an unreadable file is preserved as
/// `settings.json.corrupt` and replaced with defaults so the next start is healthy. Read I/O
/// errors other than "not found" never write (the disk is probably not writable either).
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
                    "settings: {} is not valid settings JSON ({err}); using defaults",
                    path.display()
                );
                backup_corrupt(&path);
                let settings = Settings::default();
                persist_defaults(dir, &settings);
                settings
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
                "settings: failed to read {} ({err}); using in-memory defaults",
                path.display()
            );
            Settings::default()
        }
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

/// Moves an unreadable file aside so the next read starts from a clean state.
///
/// Used for both `settings.json` and `state.json`; returns the backup path on success.
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

/// Writes one JSON document atomically: `<name>.tmp` then rename over `<name>`.
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
    let tmp = path.with_file_name(format!("{}.tmp", name.to_string_lossy()));

    let result = write_then_rename(&tmp, path, bytes);
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Writes `bytes` to `tmp`, flushes it, and renames it over `path`.
fn write_then_rename(tmp: &Path, path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
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
    fs::rename(tmp, path)
        .with_context(|| format!("failed to rename {} to {}", tmp.display(), path.display()))
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

/// Deserializes a string enum (`CardSize`, `Theme`, `KeepOnLine`), rejecting unknown values.
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
        // All 18 keys from design spec 3.6, in table order.
        assert_eq!(s.capture_hotkey, "Ctrl+Alt+S");
        assert_eq!(s.toggle_hotkey, "Ctrl+Alt+T");
        assert!(s.watch_system_screenshots);
        assert!(s.watch_clipboard);
        assert!(s.watch_recordings);
        assert!(s.extra_folders.is_empty());
        assert!(s.reveal_on_new);
        assert_eq!(s.reveal_peek_ms, 2500);
        assert!(!s.top_edge_reveal);
        assert!(!s.show_over_full_screen);
        assert_eq!(s.keep_on_line, KeepOnLine::Fit);
        assert_eq!(s.card_size, CardSize::Medium);
        assert!(s.sound);
        assert!(!s.start_at_login);
        assert_eq!(s.language, "system");
        assert_eq!(s.theme, Theme::System);
        assert!(s.auto_check_updates);
        assert_eq!(s.last_update_check, "");
    }

    #[test]
    fn default_json_has_exactly_the_eighteen_camel_case_keys() {
        let value = serde_json::to_value(Settings::default()).expect("serialize defaults");
        let object = value.as_object().expect("settings must be a JSON object");
        assert_eq!(object.len(), 18, "spec 3.6 defines exactly 18 keys");
        for key in [
            "captureHotkey",
            "toggleHotkey",
            "watchSystemScreenshots",
            "watchClipboard",
            "watchRecordings",
            "extraFolders",
            "revealOnNew",
            "revealPeekMs",
            "topEdgeReveal",
            "showOverFullScreen",
            "keepOnLine",
            "cardSize",
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
        assert_eq!(object["keepOnLine"], json!("fit"));
        assert_eq!(object["cardSize"], json!("medium"));
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

        let before = s.clone();
        assert!(s.set_json("toggleHotkey", json!("T")).is_err());
        assert!(s.set_json("toggleHotkey", json!(42)).is_err());
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
            ("revealOnNew", json!(null)),
            ("cardSize", json!("huge")),
            ("cardSize", json!(2)),
            ("theme", json!("sepia")),
            ("keepOnLine", json!(7)),
            ("keepOnLine", json!("25")),
            ("keepOnLine", json!(null)),
        ] {
            assert!(
                s.set_json(key, value.clone()).is_err(),
                "{key}={value} must be rejected"
            );
            assert_eq!(s, before, "{key} rejection must not mutate settings");
        }

        s.set_json("cardSize", json!("large")).unwrap();
        assert_eq!(s.card_size, CardSize::Large);
        s.set_json("theme", json!("dark")).unwrap();
        assert_eq!(s.theme, Theme::Dark);
        s.set_json("keepOnLine", json!(50)).unwrap();
        assert_eq!(s.keep_on_line, KeepOnLine::Limit(50));
        s.set_json("keepOnLine", json!("fit")).unwrap();
        assert_eq!(s.keep_on_line, KeepOnLine::Fit);
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
        // Everything else must keep the spec default (settings migration path).
        assert_eq!(loaded.capture_hotkey, "Ctrl+Alt+S");
        assert_eq!(loaded.reveal_peek_ms, 2500);
        assert!(loaded.reveal_on_new);
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

    /// Task 14: `set_setting` must re-apply the panel window geometry for the two keys that can
    /// change the card layout/size, and only for those (the check is a pure helper so the command
    /// path is unit-testable without a Tauri runtime).
    #[test]
    fn geometry_affecting_settings_are_card_size_and_keep_on_line() {
        assert!(affects_panel_geometry("cardSize"));
        assert!(affects_panel_geometry("keepOnLine"));
        for key in [
            "captureHotkey",
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
