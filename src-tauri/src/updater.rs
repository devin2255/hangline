//! In-app updates (P5-2): `tauri-plugin-updater`, daily/manual checks and an explicit install.
//!
//! # Network contract (design spec §6.2)
//!
//! The configured updater endpoint is the **only** network destination Hangline ever contacts:
//!
//! - an automatic check runs once per 24 h on startup, and only while `autoCheckUpdates` is on;
//! - a manual check (`Check for updates…` in the tray) is always allowed;
//! - the download/install starts only from the explicit `install_update` command (the panel's
//!   `Install update` button) and uses the URL announced by the same endpoint.
//!
//! There is no telemetry, no crash reporting and no other HTTP client. With `autoCheckUpdates`
//! off and no manual click, the app performs zero network requests. [`check`] never downloads an
//! update on its own: it only announces one.
//!
//! # Check gating
//!
//! [`should_check`] is the pure decision: manual checks always pass; an automatic check needs
//! [`should_check_auto`] (enabled, a valid `lastUpdateCheck` at least 24 h old, or a clock that
//! moved backwards). An empty/invalid timestamp is treated as "never checked" so a settings file
//! written by an older build cannot suppress updates forever. The timestamp is persisted only
//! after a check completed (with or without an update); a failed check is retried on the next
//! launch instead of being postponed for a day.
//!
//! # Events and errors
//!
//! `update-available {version, notes, pubDate, manual}`, `update-progress {chunk, total}` and the
//! manual-only `update-check-started` / `update-check-finished` / `update-check-failed` events feed
//! the panel notice. Automatic failures are logged and stay silent (spec §4.8: no system toast).
//! Manual failures are returned to the caller as a human-readable string so the panel can show
//! "Couldn't check for updates"; the documented Releases page ([`crate::actions::RELEASES_URL`])
//! remains only a fallback link in the log line.
//!
//! # Windows install
//!
//! `Update::download_and_install` verifies the minisign signature of the downloaded bytes, launches
//! the installer in the configured passive mode and exits the process; the installer restarts the
//! app. The `app.restart()` at the end of [`install`] is the documented fallback for platforms
//! where the install call returns, and is unreachable on Windows by design. A download/signature
//! failure never reaches the installer, so no partial install is claimed.
//!
//! # Capabilities
//!
//! The frontend calls our `check_updates` / `install_update` / `get_pending_update` commands, not
//! the plugin's JS API. No `updater:default` capability is granted on purpose: the plugin's JS
//! commands stay unreachable from every webview while the Rust side owns the workflow.
//!
//! # Debug-only endpoint override
//!
//! `HANG_UPDATE_ENDPOINT` replaces the configured endpoint **in debug builds only**
//! (`cfg(debug_assertions)`) so the local `scripts/release/update-check-test.mjs` harness can point
//! the app at a localhost manifest without a public release. Release builds compile the override
//! out; the env var is ignored there.

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::{Error as UpdaterError, Update, Updater, UpdaterExt};

use crate::app_state::AppState;

/// One day in milliseconds: the automatic check cadence (design spec §6.2 "每日检查一次").
pub const AUTO_CHECK_INTERVAL_MS: u64 = 24 * 60 * 60 * 1000;

/// Event: a newer release was announced by the endpoint (`UpdateInfoDto`).
pub const EVENT_UPDATE_AVAILABLE: &str = "update-available";
/// Event: download progress of an install (`UpdateProgressDto`).
pub const EVENT_UPDATE_PROGRESS: &str = "update-progress";
/// Event: a manual check started (payload `{ manual: true }`).
pub const EVENT_UPDATE_CHECK_STARTED: &str = "update-check-started";
/// Event: a manual check completed without an update (`{ available: false }`).
pub const EVENT_UPDATE_CHECK_FINISHED: &str = "update-check-finished";
/// Event: a manual check failed (`{ message }`); automatic failures stay quiet.
pub const EVENT_UPDATE_CHECK_FAILED: &str = "update-check-failed";

/// Debug-only environment variable that replaces the configured endpoint (see module docs).
#[cfg(debug_assertions)]
const DEBUG_ENDPOINT_ENV: &str = "HANG_UPDATE_ENDPOINT";

/// Update information announced by the endpoint, returned by `check_updates` and the
/// `update-available` event. `manual` records how the check was triggered so the panel can label
/// an automatic announcement differently if it ever needs to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfoDto {
    /// The announced version (e.g. `0.4.1`).
    pub version: String,
    /// Release notes from the manifest's `notes` field.
    pub notes: Option<String>,
    /// The manifest's `pub_date` verbatim (ISO 8601); `None` when the manifest omits it.
    pub pub_date: Option<String>,
    /// `true` when a manual check produced this announcement.
    pub manual: bool,
}

impl UpdateInfoDto {
    /// Maps one plugin [`Update`] to the IPC/event payload.
    fn from_update(update: &Update, manual: bool) -> Self {
        Self {
            version: update.version.clone(),
            notes: update.body.clone(),
            pub_date: manifest_pub_date(&update.raw_json),
            manual,
        }
    }
}

/// The raw manifest `pub_date` value.
///
/// The plugin already parses the date into a `time` value; reading the manifest field keeps the
/// payload exactly as published and avoids a second date library. Both the Tauri `pub_date` and a
/// defensive `pubDate` spelling are accepted; blank strings become `None`.
fn manifest_pub_date(raw_json: &serde_json::Value) -> Option<String> {
    raw_json
        .get("pub_date")
        .or_else(|| raw_json.get("pubDate"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// Download progress: `chunk` is the number of bytes downloaded so far (cumulative) and `total`
/// the server-declared content length when it provided one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateProgressDto {
    /// Cumulative bytes downloaded so far.
    pub chunk: u64,
    /// Total bytes announced by `Content-Length`, when known.
    pub total: Option<u64>,
}

/// Payload of [`EVENT_UPDATE_CHECK_STARTED`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCheckStartedDto {
    /// Always `true`: automatic checks are deliberately silent.
    pub manual: bool,
}

/// Payload of [`EVENT_UPDATE_CHECK_FINISHED`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCheckFinishedDto {
    /// Whether the check announced an update (a `false` result clears the "checking" notice).
    pub available: bool,
}

/// Payload of [`EVENT_UPDATE_CHECK_FAILED`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCheckFailedDto {
    /// Diagnostic detail; the panel shows a translated generic message.
    pub message: String,
}

/// The update most recently announced by a check.
///
/// The panel queries it once after registering its listeners (`get_pending_update`), so an
/// announcement emitted before the webview was listening is not lost. A later successful check
/// that finds no update clears the entry.
#[derive(Default)]
pub struct PendingUpdate(Mutex<Option<UpdateInfoDto>>);

impl PendingUpdate {
    /// Replaces the cached announcement (`None` = no pending update).
    fn set(&self, info: Option<UpdateInfoDto>) {
        let mut guard = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *guard = info;
    }

    /// Returns the cached announcement, if any.
    fn get(&self) -> Option<UpdateInfoDto> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

/// Why an updater operation failed, for log wording and friendly messages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateErrorKind {
    /// The endpoint could not be reached or returned no usable release metadata.
    Network,
    /// The endpoint/manifest configuration or the platform is unusable.
    Configuration,
    /// The manifest or the downloaded artifact failed validation (signature/format/version).
    Verification,
    /// Writing/downloading/launching the installer failed.
    Install,
    /// A plugin error added after this classification was written.
    Other,
}

impl UpdateErrorKind {
    /// Short stable label used in logs.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Network => "network",
            Self::Configuration => "configuration",
            Self::Verification => "verification",
            Self::Install => "install",
            Self::Other => "other",
        }
    }
}

/// Classifies one plugin error (unit-tested; the UI only shows a generic translated message).
pub fn classify_error(error: &UpdaterError) -> UpdateErrorKind {
    use tauri_plugin_updater::Error;
    match error {
        Error::Reqwest(_) | Error::Network(_) | Error::ReleaseNotFound => UpdateErrorKind::Network,
        Error::EmptyEndpoints
        | Error::UnsupportedArch
        | Error::UnsupportedOs
        | Error::UrlParse(_)
        | Error::Serialization(_)
        | Error::TargetNotFound(_)
        | Error::TargetsNotFound(_)
        | Error::InsecureTransportProtocol
        | Error::FormatDate => UpdateErrorKind::Configuration,
        Error::Minisign(_)
        | Error::Base64(_)
        | Error::SignatureUtf8(_)
        | Error::SignedVersionMismatch { .. }
        | Error::MissingSignedVersion
        | Error::InvalidUpdaterFormat => UpdateErrorKind::Verification,
        Error::Io(_)
        | Error::TempDirNotOnSameMountPoint
        | Error::PreviousAppNotRestored(_)
        | Error::BinaryNotFoundInArchive
        | Error::TempDirNotFound
        | Error::AuthenticationFailed
        | Error::DebInstallFailed
        | Error::PackageInstallFailed => UpdateErrorKind::Install,
        _ => UpdateErrorKind::Other,
    }
}

/// Human-readable check failure (the frontend additionally shows its translated generic text).
pub fn describe_check_error(error: &UpdaterError) -> String {
    format!(
        "couldn't check for updates ({}): {error}",
        classify_error(error).as_str()
    )
}

/// Human-readable install failure; no "installed" wording is ever produced on an error path.
pub fn describe_install_error(error: &UpdaterError) -> String {
    format!(
        "update install failed ({}): {error}",
        classify_error(error).as_str()
    )
}

/// Current wall-clock time in milliseconds since the Unix epoch (0 if the clock predates it).
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

/// Whether an **automatic** check may run now.
///
/// - `auto == false` → never;
/// - empty or invalid `last_iso` → yes (never checked, or a hand-edited file);
/// - a timestamp in the future (clock moved back) → yes, once, instead of waiting forever;
/// - otherwise only after at least [`AUTO_CHECK_INTERVAL_MS`].
pub fn should_check_auto(auto: bool, last_iso: &str, now_ms: u64) -> bool {
    if !auto {
        return false;
    }
    let last_iso = last_iso.trim();
    if last_iso.is_empty() {
        return true;
    }
    let Some(last_ms) = parse_iso8601_ms(last_iso) else {
        log::debug!("updater: invalid lastUpdateCheck {last_iso:?}; treating it as never checked");
        return true;
    };
    let now_ms = i64::try_from(now_ms).unwrap_or(i64::MAX);
    if last_ms > now_ms {
        log::debug!(
            "updater: lastUpdateCheck is in the future; checking once after the clock change"
        );
        return true;
    }
    (now_ms - last_ms) as u64 >= AUTO_CHECK_INTERVAL_MS
}

/// Whether a check with this trigger may run: manual checks are always allowed (spec §6.2).
pub fn should_check(manual: bool, auto: bool, last_iso: &str, now_ms: u64) -> bool {
    if manual && manual_check_allowed() {
        return true;
    }
    should_check_auto(auto, last_iso, now_ms)
}

/// Documents the manual-check invariant: a manual check never depends on the 24 h window or on
/// `autoCheckUpdates`. Kept as a named pure function so the contract is unit-tested and greppable.
pub const fn manual_check_allowed() -> bool {
    true
}

/// Persists `lastUpdateCheck` through the shared settings path (one revision + state broadcast).
///
/// A persistence failure is logged but must not turn a completed check into a failure: the update
/// announcement and the user's install button stay valid.
fn persist_last_check(app: &AppHandle) {
    let iso = format_iso8601_ms(now_ms() as i64);
    match crate::actions::apply(
        app,
        "lastUpdateCheck",
        serde_json::Value::String(iso.clone()),
    ) {
        Ok(_) => log::info!("updater: lastUpdateCheck persisted as {iso}"),
        Err(error) => log::warn!("updater: cannot persist lastUpdateCheck {iso}: {error}"),
    }
}

/// Stores/replaces the pending announcement for the panel's post-mount reconciliation.
fn set_pending(app: &AppHandle, info: Option<UpdateInfoDto>) {
    if let Some(pending) = app.try_state::<PendingUpdate>() {
        pending.set(info);
    }
}

/// Emits one updater event, logging (never failing the operation) when no window is listening.
fn emit<T: Serialize + Clone>(app: &AppHandle, event: &str, payload: &T) {
    if let Err(error) = app.emit(event, payload.clone()) {
        log::warn!("updater: cannot emit {event}: {error}");
    }
}

/// Builds an updater, applying the debug-only endpoint override when present.
fn build_updater(app: &AppHandle) -> Result<Updater, String> {
    // A release build never mutates the builder: the debug-only override is compiled out.
    #[cfg(debug_assertions)]
    let mut builder = app.updater_builder();
    #[cfg(not(debug_assertions))]
    let builder = app.updater_builder();
    #[cfg(debug_assertions)]
    if let Some(endpoint) = debug_endpoint() {
        let url = tauri::Url::parse(&endpoint)
            .map_err(|error| format!("invalid {DEBUG_ENDPOINT_ENV} {endpoint:?}: {error}"))?;
        log::warn!(
            "updater: debug endpoint override {endpoint} is active; release builds ignore \
             {DEBUG_ENDPOINT_ENV}"
        );
        builder = builder
            .endpoints(vec![url])
            .map_err(|error| describe_check_error(&error))?;
    }
    builder
        .build()
        .map_err(|error| describe_check_error(&error))
}

/// Reads the debug-only endpoint override, ignoring an empty value.
#[cfg(debug_assertions)]
fn debug_endpoint() -> Option<String> {
    let raw = std::env::var(DEBUG_ENDPOINT_ENV).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.to_string())
}

/// Checks the configured endpoint once.
///
/// `manual = false` respects [`should_check_auto`]; `manual = true` always runs. On a successful
/// check the timestamp is persisted and, when a release is announced, `update-available` is
/// emitted and the announcement is cached for `get_pending_update`; **nothing is downloaded**.
/// A manual failure returns `Err` (the panel shows a translated "couldn't check" notice); an
/// automatic failure is logged and reported as `Ok(None)` so startup stays quiet.
pub async fn check(app: &AppHandle, manual: bool) -> Result<Option<UpdateInfoDto>, String> {
    let settings = app
        .try_state::<AppState>()
        .map(|state| state.snapshot().settings)
        .unwrap_or_default();
    if !should_check(
        manual,
        settings.auto_check_updates,
        &settings.last_update_check,
        now_ms(),
    ) {
        log::debug!(
            "updater: automatic check skipped (autoCheckUpdates={}, lastUpdateCheck={:?})",
            settings.auto_check_updates,
            settings.last_update_check
        );
        return Ok(None);
    }

    if manual {
        emit(
            app,
            EVENT_UPDATE_CHECK_STARTED,
            &UpdateCheckStartedDto { manual: true },
        );
    }

    let updater = match build_updater(app) {
        Ok(updater) => updater,
        Err(error) => {
            if manual {
                log::warn!("updater: cannot build the updater: {error}");
                emit(
                    app,
                    EVENT_UPDATE_CHECK_FAILED,
                    &UpdateCheckFailedDto {
                        message: error.clone(),
                    },
                );
                return Err(error);
            }
            log::warn!("updater: cannot build the updater; automatic check skipped: {error}");
            return Ok(None);
        }
    };

    match updater.check().await {
        Ok(Some(update)) => {
            let info = UpdateInfoDto::from_update(&update, manual);
            persist_last_check(app);
            set_pending(app, Some(info.clone()));
            emit(app, EVENT_UPDATE_AVAILABLE, &info);
            log::info!(
                "updater: update {} is available (current {}; trigger={}); waiting for an explicit \
                 install",
                info.version,
                update.current_version,
                if manual { "manual" } else { "automatic" }
            );
            Ok(Some(info))
        }
        Ok(None) => {
            persist_last_check(app);
            set_pending(app, None);
            log::info!(
                "updater: no update available (trigger={})",
                if manual { "manual" } else { "automatic" }
            );
            if manual {
                emit(
                    app,
                    EVENT_UPDATE_CHECK_FINISHED,
                    &UpdateCheckFinishedDto { available: false },
                );
            }
            Ok(None)
        }
        Err(error) => {
            let kind = classify_error(&error);
            if manual {
                log::warn!("updater: manual check failed ({}): {error}", kind.as_str());
                emit(
                    app,
                    EVENT_UPDATE_CHECK_FAILED,
                    &UpdateCheckFailedDto {
                        message: error.to_string(),
                    },
                );
                Err(describe_check_error(&error))
            } else {
                log::warn!(
                    "updater: automatic check failed quietly ({}): {error}",
                    kind.as_str()
                );
                Ok(None)
            }
        }
    }
}

/// Downloads and installs the announced update after an explicit user action.
///
/// Re-checks the endpoint (the announcement is deliberately not trusted as a download URL), then
/// streams the installer with throttled [`EVENT_UPDATE_PROGRESS`] events. The signature is verified
/// by the plugin before anything is launched; on Windows the installer is started in passive mode
/// and the process exits (the installer restarts the app). Errors are returned as a friendly
/// string and never claim a partial install.
pub async fn install(app: &AppHandle) -> Result<(), String> {
    let updater = build_updater(app)?;
    let update = updater.check().await.map_err(|error| {
        let message = describe_install_error(&error);
        log::warn!("updater: install pre-check failed: {message}");
        message
    })?;
    let Some(update) = update else {
        log::info!("updater: install requested but no newer release is available");
        return Err("no update is available to install".to_string());
    };

    log::info!(
        "updater: downloading update {} (current {}) ...",
        update.version,
        update.current_version
    );
    let progress_app = app.clone();
    let mut throttle = ProgressThrottle::default();
    let result = update
        .download_and_install(
            move |chunk_length, total| {
                if let Some(progress) = throttle.record(chunk_length, total) {
                    emit(&progress_app, EVENT_UPDATE_PROGRESS, &progress);
                }
            },
            || log::info!("updater: download finished; verifying and launching the installer"),
        )
        .await;

    match result {
        Ok(()) => {
            log::info!(
                "updater: update {} installed; the installer restarts the app (the Windows install \
                 call exits the process before this point)",
                update.version
            );
            // Unreachable on Windows (the plugin launches the installer and exits). Kept as the
            // documented fallback for platforms where the install call returns normally.
            app.restart()
        }
        Err(error) => {
            let kind = classify_error(&error);
            log::warn!(
                "updater: install of {} failed ({}): {error}",
                update.version,
                kind.as_str()
            );
            Err(describe_install_error(&error))
        }
    }
}

/// Throttles the per-chunk download callback into at most one progress event per percentage point
/// (or per MiB when the server did not send a `Content-Length`).
#[derive(Default)]
struct ProgressThrottle {
    /// Cumulative bytes received.
    downloaded: u64,
    /// Last emitted percentage (known total only).
    last_percent: Option<u64>,
    /// Cumulative bytes at the last milestone emission (unknown total only).
    last_milestone: u64,
}

impl ProgressThrottle {
    /// Records one chunk and returns the event payload when this chunk should be announced.
    fn record(&mut self, chunk_length: usize, total: Option<u64>) -> Option<UpdateProgressDto> {
        self.downloaded = self
            .downloaded
            .saturating_add(u64::try_from(chunk_length).unwrap_or(u64::MAX));
        let should_emit = match total {
            Some(total) if total > 0 => {
                let percent = self.downloaded.saturating_mul(100) / total;
                let percent = percent.min(100);
                if self.last_percent == Some(percent) {
                    false
                } else {
                    self.last_percent = Some(percent);
                    true
                }
            }
            _ => {
                if self.downloaded.saturating_sub(self.last_milestone) >= 1024 * 1024 {
                    self.last_milestone = self.downloaded;
                    true
                } else {
                    false
                }
            }
        };
        should_emit.then(|| UpdateProgressDto {
            chunk: self.downloaded,
            total,
        })
    }
}

/// Starts the one-per-launch automatic check on the Tauri async runtime.
///
/// Called from `setup` after the state/pending-update stores are managed. The gating inside
/// [`check`] keeps this a no-op when `autoCheckUpdates` is off or the last successful check is
/// younger than 24 h.
pub fn spawn_startup_check(app: &AppHandle) {
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        match check(&handle, false).await {
            Ok(Some(info)) => log::info!("updater: startup check announced {}", info.version),
            Ok(None) => log::debug!("updater: startup check finished without an update"),
            Err(error) => log::warn!("updater: startup check failed quietly: {error}"),
        }
    });
}

/// Command: check for updates. `manual` bypasses the daily window (Tauri camelCase arguments).
#[tauri::command]
pub async fn check_updates(app: AppHandle, manual: bool) -> Result<Option<UpdateInfoDto>, String> {
    check(&app, manual).await
}

/// Command: download and install the announced update, then let the installer restart the app.
#[tauri::command]
pub async fn install_update(app: AppHandle) -> Result<(), String> {
    install(&app).await
}

/// Command: the update announced before the panel registered its listeners (or `null`).
#[tauri::command]
pub fn get_pending_update(app: AppHandle) -> Option<UpdateInfoDto> {
    app.try_state::<PendingUpdate>()
        .and_then(|pending| pending.get())
}

/// Parses the subset of ISO 8601 that Hangline writes and accepts for `lastUpdateCheck`:
/// `YYYY-MM-DDTHH:MM:SS[.fraction][Z|±HH:MM]`, with a space/t as the separator and an empty suffix
/// meaning UTC. Calendar fields are validated (leap years included). Returns epoch milliseconds.
pub fn parse_iso8601_ms(value: &str) -> Option<i64> {
    let bytes = value.as_bytes();
    if !value.is_ascii() || bytes.len() < 19 {
        return None;
    }
    let digits = |from: usize, to: usize| -> Option<u32> {
        let slice = bytes.get(from..to)?;
        if !slice.iter().all(u8::is_ascii_digit) {
            return None;
        }
        std::str::from_utf8(slice).ok()?.parse().ok()
    };
    let at = |index: usize, byte: u8| bytes.get(index) == Some(&byte);

    let year = digits(0, 4)? as i64;
    let month = digits(5, 7)?;
    let day = digits(8, 10)?;
    let separator = bytes.get(10).copied()?;
    if !matches!(separator, b'T' | b't' | b' ') || !at(4, b'-') || !at(7, b'-') {
        return None;
    }
    let hour = digits(11, 13)?;
    let minute = digits(14, 16)?;
    let second = digits(17, 19)?;
    if !at(13, b':') || !at(16, b':') {
        return None;
    }
    if !(1..=12).contains(&month)
        || day == 0
        || day > days_in_month(year, month)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return None;
    }

    let mut rest = &value[19..];
    let mut millis = 0i64;
    if let Some(fraction) = rest.strip_prefix('.') {
        let digits = fraction.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 || digits > 9 {
            return None;
        }
        let fraction_digits = &fraction[..digits];
        // Milliseconds = the first three digits, zero-padded.
        let mut value = 0i64;
        for index in 0..3 {
            value *= 10;
            if let Some(byte) = fraction_digits.as_bytes().get(index) {
                value += i64::from(byte - b'0');
            }
        }
        millis = value;
        rest = &fraction[digits..];
    }

    let offset_minutes: i64 = if rest.is_empty() || rest.eq_ignore_ascii_case("Z") {
        0
    } else {
        let offset = rest.as_bytes();
        if offset.len() != 6
            || !matches!(offset[0], b'+' | b'-')
            || !offset[1..3].iter().all(u8::is_ascii_digit)
            || offset[3] != b':'
            || !offset[4..6].iter().all(u8::is_ascii_digit)
        {
            return None;
        }
        let hours = i64::from((offset[1] - b'0') * 10 + (offset[2] - b'0'));
        let minutes = i64::from((offset[4] - b'0') * 10 + (offset[5] - b'0'));
        if hours > 23 || minutes > 59 {
            return None;
        }
        let magnitude = hours * 60 + minutes;
        if offset[0] == b'+' {
            magnitude
        } else {
            -magnitude
        }
    };

    let days = days_from_civil(year, month, day);
    let seconds = days
        .checked_mul(86_400)?
        .checked_add(i64::from(hour) * 3_600 + i64::from(minute) * 60 + i64::from(second))?;
    seconds
        .checked_mul(1_000)?
        .checked_add(millis)?
        .checked_sub(offset_minutes * 60_000)
}

/// Formats epoch milliseconds as `YYYY-MM-DDTHH:MM:SS.mmmZ` (the shape settings validation and
/// the 24 h gate accept). Year values outside four digits are clamped only by formatting width;
/// real wall-clock years are always four digits.
pub fn format_iso8601_ms(ms: i64) -> String {
    let seconds = ms.div_euclid(1_000);
    let millis = ms.rem_euclid(1_000);
    let days = seconds.div_euclid(86_400);
    let day_seconds = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = day_seconds / 3_600;
    let minute = (day_seconds % 3_600) / 60;
    let second = day_seconds % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

/// Number of days in `month` of `year` (proleptic Gregorian calendar).
fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

/// Proleptic Gregorian leap-year rule.
fn is_leap_year(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

/// Days since 1970-01-01 for a Gregorian date (Howard Hinnant's `days_from_civil`).
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = (if year >= 0 { year } else { year - 399 }).div_euclid(400);
    let year_of_era = year - era * 400;
    let month_prime = i64::from((month + 9) % 12);
    let day_of_year = (153 * month_prime + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The Gregorian date for days since 1970-01-01 (Howard Hinnant's `civil_from_days`).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }).div_euclid(146_097);
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    };
    let year = if month <= 2 { year + 1 } else { year };
    (year, month as u32, day as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    use serde_json::json;

    /// A fixed instant: 2026-11-04T00:00:00Z.
    const NOW: u64 = 1_793_750_400_000;

    #[test]
    fn automatic_check_respects_auto_flag() {
        assert!(!should_check_auto(false, "", NOW));
        assert!(!should_check_auto(false, "2020-01-01T00:00:00Z", NOW));
        assert!(should_check_auto(true, "", NOW));
    }

    #[test]
    fn automatic_check_treats_empty_or_invalid_timestamps_as_never_checked() {
        assert!(should_check_auto(true, "", NOW));
        assert!(should_check_auto(true, "   ", NOW));
        assert!(should_check_auto(true, "not a timestamp", NOW));
        // Calendar-invalid values are also "invalid" (a hand-edited settings file).
        assert!(should_check_auto(true, "2026-02-30T00:00:00Z", NOW));
        assert!(should_check_auto(true, "2026-13-01T00:00:00Z", NOW));
    }

    #[test]
    fn automatic_check_uses_the_24_hour_boundary() {
        // One millisecond short of 24 h: skip.
        assert!(!should_check_auto(true, "2026-11-03T00:00:00.001Z", NOW));
        // Exactly 24 h: run (>= boundary).
        assert!(should_check_auto(true, "2026-11-03T00:00:00Z", NOW));
        assert!(should_check_auto(true, "2026-11-03T00:00:00.000Z", NOW));
        // Older than 24 h: run.
        assert!(should_check_auto(true, "2026-11-02T23:59:59Z", NOW));
    }

    #[test]
    fn automatic_check_runs_once_after_the_clock_moves_back() {
        // The stored check is in the future: immediately run once instead of waiting it out.
        assert!(should_check_auto(true, "2026-11-05T00:00:00Z", NOW));
        assert!(should_check_auto(true, "2030-01-01T00:00:00Z", NOW));
        // But `autoCheckUpdates = false` still wins.
        assert!(!should_check_auto(false, "2030-01-01T00:00:00Z", NOW));
    }

    #[test]
    fn manual_checks_are_always_allowed() {
        assert!(manual_check_allowed());
        assert!(should_check(true, false, "", NOW));
        assert!(should_check(true, false, "2026-11-04T00:00:00Z", NOW));
        assert!(should_check(true, true, "2030-01-01T00:00:00Z", NOW));
        // Automatic checks still follow the gate.
        assert!(!should_check(false, false, "", NOW));
        assert!(!should_check(false, true, "2026-11-04T00:00:00Z", NOW));
    }

    #[test]
    fn iso_parser_accepts_the_shapes_hangline_writes() {
        assert_eq!(parse_iso8601_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_iso8601_ms("1970-01-01T00:00:00.123Z"), Some(123));
        assert_eq!(parse_iso8601_ms("1970-01-01T00:00:00.1Z"), Some(100));
        assert_eq!(
            parse_iso8601_ms("1970-01-01T00:00:00.123456789Z"),
            Some(123)
        );
        // Space/t separators and a missing suffix (= UTC) are accepted by settings validation too.
        assert_eq!(parse_iso8601_ms("1970-01-01 00:00:00"), Some(0));
        assert_eq!(parse_iso8601_ms("1970-01-01t00:00:00"), Some(0));
        // A positive offset means local time ahead of UTC.
        assert_eq!(
            parse_iso8601_ms("1970-01-01T02:30:00+02:30"),
            Some(0),
            "UTC 00:00"
        );
        assert_eq!(
            parse_iso8601_ms("1969-12-31T19:00:00-05:00"),
            Some(0),
            "UTC 00:00"
        );
        // Known epoch and leap-day values.
        assert_eq!(parse_iso8601_ms("2026-11-04T00:00:00Z"), Some(NOW as i64));
        assert_eq!(
            parse_iso8601_ms("2024-02-29T00:00:00Z"),
            Some(1_709_164_800_000)
        );
        assert_eq!(
            parse_iso8601_ms("2026-11-04T00:00:00.500+08:00"),
            Some(NOW as i64 - 8 * 3_600_000 + 500)
        );
    }

    #[test]
    fn iso_parser_rejects_malformed_values() {
        for value in [
            "",
            "2026-11-04",
            "2026-11-04T00:00",
            "2026-11-04T00:00:00.",
            "2026-11-04T00:00:00.Z",
            "2026-11-04T00:00:00+2:00",
            "2026-11-04T00:00:00+02:0",
            "2026-11-04T00:00:00+25:00",
            "2026-11-04T00:00:00+02:60",
            "2026-11-04T24:00:00Z",
            "2026-11-04T00:60:00Z",
            "2026-11-04T00:00:60Z",
            "2026-13-04T00:00:00Z",
            "2026-00-04T00:00:00Z",
            "2026-11-00T00:00:00Z",
            "2026-11-32T00:00:00Z",
            "2023-02-29T00:00:00Z",
            "2026-11-04X00:00:00Z",
            "2026-11-04T00:00:00Zjunk",
            "2026-11-04T00:00:00.1234567890Z",
            "２０２６-11-04T00:00:00Z",
        ] {
            assert_eq!(parse_iso8601_ms(value), None, "{value:?} must be rejected");
        }
    }

    #[test]
    fn iso_formatter_round_trips_and_matches_the_settings_validator() {
        for ms in [
            NOW as i64,
            NOW as i64 + 1,
            NOW as i64 + 86_399_999,
            0,
            1_709_164_800_000,
            1_709_164_800_001,
        ] {
            let text = format_iso8601_ms(ms);
            assert_eq!(parse_iso8601_ms(&text), Some(ms), "{text} must round-trip");
            let mut settings = crate::settings::Settings::default();
            assert!(
                settings.set_json("lastUpdateCheck", json!(text)).is_ok(),
                "{text} must be accepted by settings validation"
            );
        }
        assert_eq!(format_iso8601_ms(NOW as i64), "2026-11-04T00:00:00.000Z");
        // Before the epoch still formats the four-digit year and round-trips.
        assert_eq!(format_iso8601_ms(-1), "1969-12-31T23:59:59.999Z");
        assert_eq!(parse_iso8601_ms("1969-12-31T23:59:59.999Z"), Some(-1));
    }

    #[test]
    fn leap_year_boundaries_are_validated() {
        assert!(parse_iso8601_ms("2000-02-29T00:00:00Z").is_some());
        assert!(parse_iso8601_ms("1900-02-29T00:00:00Z").is_none());
        assert!(parse_iso8601_ms("2028-02-29T00:00:00Z").is_some());
    }

    #[test]
    fn update_info_maps_the_manifest_fields_and_serializes_camel_case() {
        let raw = json!({
            "version": "0.4.1",
            "notes": "Fixes",
            "pub_date": " 2026-11-05T00:00:00Z ",
            "platforms": { "windows-x86_64": { "signature": "sig", "url": "https://example.test/a.exe" } }
        });
        assert_eq!(
            manifest_pub_date(&raw),
            Some("2026-11-05T00:00:00Z".to_string())
        );
        assert_eq!(manifest_pub_date(&json!({})), None);
        assert_eq!(manifest_pub_date(&json!({ "pub_date": "   " })), None);
        assert_eq!(
            manifest_pub_date(&json!({ "pubDate": "2026-11-05T00:00:00Z" })),
            Some("2026-11-05T00:00:00Z".to_string())
        );

        let dto = UpdateInfoDto {
            version: "0.4.1".to_string(),
            notes: Some("Fixes".to_string()),
            pub_date: Some("2026-11-05T00:00:00Z".to_string()),
            manual: true,
        };
        assert_eq!(
            serde_json::to_value(&dto).expect("serialize the DTO"),
            json!({
                "version": "0.4.1",
                "notes": "Fixes",
                "pubDate": "2026-11-05T00:00:00Z",
                "manual": true
            })
        );
        let no_notes = UpdateInfoDto {
            version: "0.4.1".to_string(),
            notes: None,
            pub_date: None,
            manual: false,
        };
        assert_eq!(
            serde_json::to_value(&no_notes).expect("serialize the DTO"),
            json!({
                "version": "0.4.1",
                "notes": null,
                "pubDate": null,
                "manual": false
            })
        );
    }

    #[test]
    fn error_classification_separates_network_config_verification_and_install() {
        assert_eq!(
            classify_error(&UpdaterError::ReleaseNotFound),
            UpdateErrorKind::Network
        );
        assert_eq!(
            classify_error(&UpdaterError::Network("connection refused".into())),
            UpdateErrorKind::Network
        );
        assert_eq!(
            classify_error(&UpdaterError::UnsupportedOs),
            UpdateErrorKind::Configuration
        );
        assert_eq!(
            classify_error(&UpdaterError::TargetNotFound("windows-x86_64".into())),
            UpdateErrorKind::Configuration
        );
        assert_eq!(
            classify_error(&UpdaterError::SignatureUtf8("bad".into())),
            UpdateErrorKind::Verification
        );
        assert_eq!(
            classify_error(&UpdaterError::InvalidUpdaterFormat),
            UpdateErrorKind::Verification
        );
        assert_eq!(
            classify_error(&UpdaterError::MissingSignedVersion),
            UpdateErrorKind::Verification
        );
        assert_eq!(
            classify_error(&UpdaterError::TempDirNotFound),
            UpdateErrorKind::Install
        );
        assert_eq!(
            classify_error(&UpdaterError::PackageInstallFailed),
            UpdateErrorKind::Install
        );
        let message = describe_check_error(&UpdaterError::ReleaseNotFound);
        assert!(message.starts_with("couldn't check for updates (network):"));
        let message = describe_install_error(&UpdaterError::TempDirNotFound);
        assert!(message.starts_with("update install failed (install):"));
        assert!(message.contains("temporary directory"));
        assert_eq!(UpdateErrorKind::Network.as_str(), "network");
        assert_eq!(UpdateErrorKind::Other.as_str(), "other");
    }

    #[test]
    fn progress_throttle_emits_once_per_percent_when_total_is_known() {
        let mut throttle = ProgressThrottle::default();
        assert_eq!(
            throttle.record(30, Some(100)),
            Some(UpdateProgressDto {
                chunk: 30,
                total: Some(100)
            })
        );
        // Same percentage: no event.
        assert_eq!(throttle.record(0, Some(100)), None);
        assert_eq!(
            throttle.record(31, Some(100)),
            Some(UpdateProgressDto {
                chunk: 61,
                total: Some(100)
            })
        );
        assert_eq!(
            throttle.record(39, Some(100)),
            Some(UpdateProgressDto {
                chunk: 100,
                total: Some(100)
            })
        );
        // The cumulative counter never goes backwards and saturates.
        assert_eq!(throttle.record(10, Some(100)), None);
    }

    #[test]
    fn progress_throttle_uses_one_mib_milestones_without_a_total() {
        let mut throttle = ProgressThrottle::default();
        assert_eq!(throttle.record(512 * 1024, None), None);
        // The first crossing of 1 MiB emits; later chunks below the next milestone stay quiet.
        assert_eq!(
            throttle.record(512 * 1024, None),
            Some(UpdateProgressDto {
                chunk: 1024 * 1024,
                total: None
            })
        );
        assert_eq!(throttle.record(1024 * 1024 - 1, None), None);
        assert_eq!(
            throttle.record(1, None),
            Some(UpdateProgressDto {
                chunk: 2 * 1024 * 1024,
                total: None
            })
        );
    }

    #[test]
    fn pending_update_store_round_trips_and_clears() {
        let pending = PendingUpdate::default();
        assert_eq!(pending.get(), None);
        let info = UpdateInfoDto {
            version: "0.4.1".to_string(),
            notes: None,
            pub_date: None,
            manual: false,
        };
        pending.set(Some(info.clone()));
        assert_eq!(pending.get(), Some(info));
        pending.set(None);
        assert_eq!(pending.get(), None);
    }

    #[test]
    fn event_payloads_use_camel_case() {
        assert_eq!(
            serde_json::to_value(UpdateCheckStartedDto { manual: true }).unwrap(),
            json!({ "manual": true })
        );
        assert_eq!(
            serde_json::to_value(UpdateCheckFinishedDto { available: false }).unwrap(),
            json!({ "available": false })
        );
        assert_eq!(
            serde_json::to_value(UpdateCheckFailedDto {
                message: "boom".to_string()
            })
            .unwrap(),
            json!({ "message": "boom" })
        );
        assert_eq!(
            serde_json::to_value(UpdateProgressDto {
                chunk: 42,
                total: Some(100)
            })
            .unwrap(),
            json!({ "chunk": 42, "total": 100 })
        );
    }
}
