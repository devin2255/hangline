//! Global hotkeys: accelerator normalization and conflict-safe (re)registration.
//!
//! Registration deliberately registers the new shortcut first and only then releases the
//! previous one: when the new combination is already taken by another application the old
//! shortcut stays live, which is the rollback behaviour required by the design spec.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};

use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
use thiserror::Error;
use windows::Win32::UI::WindowsAndMessaging::{
    MB_ICONWARNING, MB_OK, MB_SETFOREGROUND, MB_TOPMOST,
};

use crate::i18n;
use crate::reveal::{self, RevealReason};

/// Default capture accelerator (design spec 3.6).
pub const DEFAULT_CAPTURE: &str = "Ctrl+Alt+S";
/// Default show/hide accelerator (design spec 3.6).
pub const DEFAULT_TOGGLE: &str = "Ctrl+Alt+T";
/// Default "edit the newest card" accelerator (Wave 1 W1.8; avoids QQ/WeChat/Snipaste defaults).
pub const DEFAULT_EDIT_LATEST: &str = "Ctrl+Alt+E";
/// Default "open search" accelerator (Wave 2 W2.3).
pub const DEFAULT_SEARCH: &str = "Ctrl+Alt+F";

/// Which action a global shortcut is bound to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Target {
    /// Capture request; temporary wiring until the capture overlay exists (Task 16).
    Capture,
    /// Show or hide the Hangline panel.
    Toggle,
    /// Open the newest card in its editor (markup for images, trim for recordings; W1.8).
    EditLatest,
    /// Open the on-demand search window (Wave 2 W2.3).
    Search,
}

impl Target {
    /// The lowercase name used in IPC payloads and logs.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Capture => "capture",
            Self::Toggle => "toggle",
            Self::EditLatest => "editLatest",
            Self::Search => "search",
        }
    }

    /// The spec-default accelerator, used when the persisted value is unparsable.
    pub fn default_accelerator(self) -> &'static str {
        match self {
            Self::Capture => DEFAULT_CAPTURE,
            Self::Toggle => DEFAULT_TOGGLE,
            Self::EditLatest => DEFAULT_EDIT_LATEST,
            Self::Search => DEFAULT_SEARCH,
        }
    }

    /// Parses the `set_hotkey` IPC `target` argument.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "capture" => Some(Self::Capture),
            "toggle" => Some(Self::Toggle),
            "editLatest" => Some(Self::EditLatest),
            "search" => Some(Self::Search),
            _ => None,
        }
    }

    /// The user-facing label from the frozen tray table (tray rows + W1.7 failure dialog).
    pub fn label(self, strings: &i18n::TrayStrings) -> &'static str {
        match self {
            Self::Capture => strings.shortcut_capture,
            Self::Toggle => strings.shortcut_toggle,
            Self::EditLatest => strings.shortcut_edit_latest,
            Self::Search => strings.shortcut_search,
        }
    }
}

/// Why an accelerator string was rejected.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum HotkeyError {
    /// The accelerator was empty (or only whitespace).
    #[error("hotkey is empty")]
    Empty,
    /// An empty token was found (`Ctrl++Alt+S`).
    #[error("hotkey has an empty token: {0:?}")]
    InvalidFormat(String),
    /// No main key was found after the modifiers (`Ctrl+Alt`).
    #[error("hotkey is missing a main key: {0:?}")]
    MissingKey(String),
    /// Fewer than two modifiers were supplied (`Ctrl+S`, `S`).
    #[error("hotkey needs at least two modifiers and one main key: {0:?}")]
    TooFewModifiers(String),
    /// The main key was not recognised.
    #[error("unknown key {0:?} in {1:?}")]
    UnknownKey(String, String),
    /// More than one main key was supplied (`Ctrl+Alt+S+T`).
    #[error("hotkey has more than one main key: {0:?}")]
    MultipleKeys(String),
    /// The OS/plugin refused the registration (usually: owned by another application).
    #[error("hotkey registration failed: {0}")]
    Register(String),
}

/// Normalizes a human-supplied accelerator into the canonical registration form.
///
/// Accepts any modifier case and order (`ctrl+alt+s`, `ALT+CTRL+T`) and returns modifiers in
/// the fixed order `Ctrl+Alt+Shift+Super` followed by the upper-cased main key, e.g.
/// `Ctrl+Alt+S`. At least two modifiers and exactly one main key are required.
pub fn normalize(accel: &str) -> Result<String, HotkeyError> {
    let raw = accel.trim();
    if raw.is_empty() {
        return Err(HotkeyError::Empty);
    }

    let mut ctrl = false;
    let mut alt = false;
    let mut shift = false;
    let mut sup = false;
    let mut key: Option<String> = None;

    for token in raw.split('+').map(str::trim) {
        if token.is_empty() {
            return Err(HotkeyError::InvalidFormat(raw.to_string()));
        }
        match token.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => ctrl = true,
            "alt" | "option" => alt = true,
            "shift" => shift = true,
            // `Super` is the canonical name understood by the global-shortcut parser on
            // Windows (it does not accept the literal `Win`).
            "win" | "windows" | "super" | "meta" | "cmd" | "command" => sup = true,
            _ => {
                if key.is_some() {
                    return Err(HotkeyError::MultipleKeys(raw.to_string()));
                }
                key = Some(canonical_key(token, raw)?);
            }
        }
    }

    let Some(key) = key else {
        return Err(HotkeyError::MissingKey(raw.to_string()));
    };
    let modifiers = [ctrl, alt, shift, sup]
        .iter()
        .filter(|is_on| **is_on)
        .count();
    if modifiers < 2 {
        return Err(HotkeyError::TooFewModifiers(raw.to_string()));
    }

    let mut normalized = String::with_capacity(24);
    for (is_on, name) in [(ctrl, "Ctrl"), (alt, "Alt"), (shift, "Shift"), (sup, "Super")] {
        if is_on {
            normalized.push_str(name);
            normalized.push('+');
        }
    }
    normalized.push_str(&key);
    Ok(normalized)
}

/// Maps a main-key token to the spelling accepted by `global_hotkey`'s parser.
fn canonical_key(token: &str, raw: &str) -> Result<String, HotkeyError> {
    let upper = token.to_ascii_uppercase();

    // Letters and digits are registered by their single-character names (`S`, `5`).
    if upper.len() == 1 {
        let byte = upper.as_bytes()[0];
        if byte.is_ascii_uppercase() || byte.is_ascii_digit() {
            return Ok(upper);
        }
    }

    if let Some(number) = upper
        .strip_prefix('F')
        .and_then(|rest| rest.parse::<u8>().ok())
    {
        if (1..=24).contains(&number) {
            return Ok(format!("F{number}"));
        }
    }

    let named = match upper.as_str() {
        "SPACE" => "Space",
        "TAB" => "Tab",
        "ENTER" | "RETURN" => "Enter",
        "ESC" | "ESCAPE" => "Escape",
        "BACKSPACE" => "Backspace",
        "DELETE" | "DEL" => "Delete",
        "INSERT" | "INS" => "Insert",
        "HOME" => "Home",
        "END" => "End",
        "PAGEUP" | "PGUP" => "PageUp",
        "PAGEDOWN" | "PGDN" => "PageDown",
        "ARROWUP" | "UP" => "ArrowUp",
        "ARROWDOWN" | "DOWN" => "ArrowDown",
        "ARROWLEFT" | "LEFT" => "ArrowLeft",
        "ARROWRIGHT" | "RIGHT" => "ArrowRight",
        "PRINTSCREEN" | "PRTSC" => "PrintScreen",
        "CAPSLOCK" => "CapsLock",
        _ => return Err(HotkeyError::UnknownKey(token.to_string(), raw.to_string())),
    };
    Ok(named.to_string())
}

/// Accelerators registered and the last registration failure, keyed by target.
///
/// Empty until [`init`] registers the persisted settings. [`register`] uses the registered map to
/// release the previous accelerator only after the new one has been accepted; the failure map is
/// what the tray turns into a disabled hint item and a tooltip suffix (Tauri's `TrayIcon` has no
/// balloon API and Task 15 adds no notification dependency, so a visible menu row replaces the
/// spec's "气泡提示").
#[derive(Debug, Default)]
pub struct Hotkeys {
    registered: Mutex<HashMap<Target, String>>,
    failures: Mutex<HashMap<Target, HotkeyFailure>>,
}

/// One shortcut that could not be registered (kept for the tray's status item).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HotkeyFailure {
    /// Which shortcut failed.
    pub target: Target,
    /// The accelerator that was rejected.
    pub accelerator: String,
    /// The OS/plugin error message.
    pub message: String,
}

impl Hotkeys {
    fn current(&self, target: Target) -> Option<String> {
        self.lock().get(&target).cloned()
    }

    fn take(&self, target: Target) -> Option<String> {
        self.lock().remove(&target)
    }

    fn set(&self, target: Target, accelerator: String) {
        self.lock().insert(target, accelerator);
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<Target, String>> {
        // A panic while holding the lock must not permanently disable hotkey changes.
        self.registered
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Records a failed registration for the tray hint item.
    fn record_failure(&self, failure: HotkeyFailure) {
        self.failures
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(failure.target, failure);
    }

    /// Clears the failure record for a target that registered successfully.
    fn clear_failure(&self, target: Target) {
        self.failures
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&target);
    }

    /// The current failures, in target order (capture → toggle → edit latest → search) for
    /// deterministic tray hint and startup-dialog order.
    fn failures(&self) -> Vec<HotkeyFailure> {
        let failures = self
            .failures
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        [
            Target::Capture,
            Target::Toggle,
            Target::EditLatest,
            Target::Search,
        ]
        .iter()
        .filter_map(|target| failures.get(target).cloned())
        .collect()
    }
}

/// Returns the first registration failure (capture before toggle before edit latest before search),
/// for the tray hint/tooltip.
pub fn failure(app: &AppHandle) -> Option<HotkeyFailure> {
    all_failures(app).into_iter().next()
}

/// Every registration failure, in target order (the W1.7 startup dialog lists them all).
pub fn all_failures(app: &AppHandle) -> Vec<HotkeyFailure> {
    app.try_state::<Hotkeys>()
        .map(|state| state.failures())
        .unwrap_or_default()
}

/// The four `(target, accelerator)` pairs a startup must register, from the persisted settings.
///
/// This is the Task 15 fix for Task 3's "register the in-memory defaults": the source is now the
/// user's `settings.json` (`captureHotkey`/`toggleHotkey`/`editLatestHotkey`/`searchHotkey`,
/// normalized by `Settings::set_json` when they were written).
pub fn configured_accelerators(settings: &crate::settings::Settings) -> [(Target, String); 4] {
    [
        (Target::Capture, settings.capture_hotkey.clone()),
        (Target::Toggle, settings.toggle_hotkey.clone()),
        (Target::EditLatest, settings.edit_latest_hotkey.clone()),
        (Target::Search, settings.search_hotkey.clone()),
    ]
}

/// Whether a `set_hotkey` request repeats the accelerator that is already persisted.
///
/// Saving the accelerator that is already in `settings.json` is a *retry*, not a no-op: startup
/// registration can have failed while another application owned the combination (design spec §3.11,
/// M15), and `actions::apply` schedules no re-registration when the value did not change. The
/// caller must therefore re-attempt registration. Both sides are normalized, so `ctrl+alt+t`
/// counts as the persisted `Ctrl+Alt+T`; an invalid value is never a retry (validation reports it).
pub fn is_registration_retry(persisted: &str, requested: &str) -> bool {
    match (normalize(persisted), normalize(requested)) {
        (Ok(persisted), Ok(requested)) => persisted == requested,
        _ => false,
    }
}

/// Registers the persisted shortcuts and starts managing hotkey state.
///
/// Called once from `setup` after the settings are loaded (an unmanaged `AppState` falls back to the
/// spec defaults). A conflicting or unparsable shortcut is logged, reported through the
/// `hotkey-error` event and (for an unparsable persisted value) replaced by the spec default; it
/// never aborts startup, the tray and the other shortcut still work. Failures stay recorded in
/// [`Hotkeys`] so the tray can show a disabled hint item (design spec §3.11: keep the old key).
pub fn init(app: &AppHandle) {
    app.manage(Hotkeys::default());
    let settings = app
        .try_state::<crate::app_state::AppState>()
        .map(|state| state.snapshot().settings)
        .unwrap_or_default();
    for (target, accelerator) in configured_accelerators(&settings) {
        if let Err(error) = register(app, target, &accelerator) {
            // A hand-edited settings file can hold an accelerator the parser rejects (not a
            // registration conflict). Fall back to the spec default so the shortcut is not dead.
            if !matches!(error, HotkeyError::Register(_))
                && accelerator != target.default_accelerator()
            {
                log::warn!(
                    "hotkeys: persisted {} shortcut {accelerator:?} is invalid ({error}); \
                     falling back to {}",
                    target.as_str(),
                    target.default_accelerator()
                );
                let _ = register(app, target, target.default_accelerator());
            }
        }
    }
}

/// Registers `accel` for `target`, replacing the target's previous accelerator.
///
/// The previous shortcut is unregistered only after the new one is live, so a conflict
/// (another application owns the combination) leaves the old shortcut working. Failures are
/// logged and broadcast as `hotkey-error`; they are never panics.
pub fn register(app: &AppHandle, target: Target, accel: &str) -> Result<(), HotkeyError> {
    let normalized = normalize(accel)?;

    let Some(state) = app.try_state::<Hotkeys>() else {
        return Err(HotkeyError::Register(
            "hotkey state is not managed".to_string(),
        ));
    };
    if state.current(target).as_deref() == Some(normalized.as_str()) {
        return Ok(());
    }

    let action_target = target;
    if let Err(err) = app.global_shortcut().on_shortcut(
        normalized.as_str(),
        move |app, _shortcut, event| {
            // Firing on release as well would toggle twice per key press.
            if event.state == ShortcutState::Pressed {
                handle_action(app, action_target);
            }
        },
    ) {
        let message = err.to_string();
        log::error!(
            "hotkeys: failed to register {} shortcut {normalized}: {message}",
            target.as_str()
        );
        // Keep the failure for the tray's disabled hint item and tooltip (no balloon API).
        state.record_failure(HotkeyFailure {
            target,
            accelerator: normalized.clone(),
            message: message.clone(),
        });
        emit_error(app, target, &normalized, &message);
        return Err(HotkeyError::Register(message));
    }

    if let Some(previous) = state.take(target) {
        if previous != normalized {
            if let Err(err) = app.global_shortcut().unregister(previous.as_str()) {
                log::warn!(
                    "hotkeys: failed to unregister previous {} shortcut {previous}: {err}",
                    target.as_str()
                );
            }
        }
    }
    state.set(target, normalized.clone());
    state.clear_failure(target);
    log::info!(
        "hotkeys: registered {} shortcut {normalized}",
        target.as_str()
    );
    Ok(())
}

/// Runs the action bound to `target` (called from the global-shortcut handler).
fn handle_action(app: &AppHandle, target: Target) {
    match target {
        Target::Capture => {
            // Task 16: the real freeze-frame overlay (design spec §3.2 A). The global-shortcut
            // plugin dispatches this handler from the Tauri event-loop thread, and creating the
            // overlay `WebviewWindow` from that thread deadlocks (the builder dispatches to the
            // event loop and waits). The handler therefore never calls `request_capture` directly:
            // a named worker thread does, exactly like the async command / tray paths (T16 review).
            log::info!("hotkeys: capture requested (dispatching to the capture worker thread)");
            crate::capture::request_capture_in_background(app);
        }
        Target::Toggle => {
            log::info!("hotkeys: toggle line");
            reveal::toggle(app, RevealReason::Toggle);
        }
        Target::EditLatest => {
            // Window creation dispatches to the event loop and waits, so the request must never run
            // on the global-shortcut/event-loop thread; the worker also runs the media duration
            // probe for a recording. The target is resolved there from the live state.
            log::info!("hotkeys: edit latest requested (dispatching to the editor worker thread)");
            crate::actions::edit_latest_in_background(app);
        }
        Target::Search => {
            // Same event-loop rule: the search window is created on a named worker thread (W2.2).
            log::info!("hotkeys: search requested (dispatching to the search window worker thread)");
            crate::search_window::open_in_background(app);
        }
    }
}

/// Payload of the `hotkey-error` event: the machine-readable registration failure.
///
/// No webview listens to it today (the tray's disabled hint row and tooltip already show the
/// failure); it is kept for the Wave 1 settings explainability instead of being deleted.
#[derive(Clone, serde::Serialize)]
struct HotkeyErrorPayload<'a> {
    target: &'a str,
    accelerator: &'a str,
    message: &'a str,
}

fn emit_error(app: &AppHandle, target: Target, accelerator: &str, message: &str) {
    let payload = HotkeyErrorPayload {
        target: target.as_str(),
        accelerator,
        message,
    };
    // No consumer today: kept for Wave 1 settings explainability, not for the IPC count.
    if let Err(err) = app.emit("hotkey-error", payload) {
        log::warn!("hotkeys: failed to emit hotkey-error: {err}");
    }
}

/// Builds the one-shot startup warning body (pure; the frozen table owns every label).
///
/// One `• <label> (<accelerator>): <reason>` line per failed shortcut, then the tray hint. The
/// dialog and the tray status row therefore explain the same failures with the same wording.
fn failure_dialog_body(strings: &i18n::TrayStrings, failures: &[HotkeyFailure]) -> String {
    let mut body = String::from(strings.hotkey_error_intro);
    for failure in failures {
        body.push('\n');
        body.push_str(&strings.hotkey_error_detail_line(
            failure.target.label(strings),
            &failure.accelerator,
            &failure.message,
        ));
    }
    body.push_str("\n\n");
    body.push_str(strings.hotkey_error_hint);
    body
}

/// W1.7: one proactive native warning per run when startup registration failed.
///
/// The tray's disabled hint row and tooltip stay the persistent surface (and `hotkey-error` stays
/// emitted); this dialog is the *proactive* one: it names every failed shortcut, the OS error and
/// where to change it. At most once per process, on a worker thread, so a modal box can never block
/// startup or window creation. An empty failure list shows nothing.
pub fn notify_startup_failures(app: &AppHandle) {
    static SHOWN: AtomicBool = AtomicBool::new(false);
    if SHOWN.swap(true, Ordering::Relaxed) {
        return;
    }
    let failures = all_failures(app);
    if failures.is_empty() {
        return;
    }
    let strings = i18n::tray_strings(i18n::app_language(app));
    let title = strings.hotkey_error_title.to_string();
    let body = failure_dialog_body(strings, &failures);
    log::warn!(
        "hotkeys: {} shortcut(s) could not be registered; showing the startup warning dialog",
        failures.len()
    );
    let spawned = std::thread::Builder::new()
        .name("hangline-hotkey-warning".to_string())
        .spawn(move || {
            crate::message_box(
                &title,
                &body,
                MB_OK | MB_ICONWARNING | MB_SETFOREGROUND | MB_TOPMOST,
            );
        });
    if let Err(error) = spawned {
        log::error!("hotkeys: cannot spawn the startup warning dialog thread: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_case_and_order() {
        assert_eq!(normalize("ctrl+alt+s").unwrap(), "Ctrl+Alt+S");
        assert_eq!(normalize("ALT+CTRL+T").unwrap(), "Ctrl+Alt+T");
        assert!(normalize("S").is_err());
    }

    #[test]
    fn requires_two_modifiers_and_one_key() {
        assert!(normalize("ctrl+s").is_err());
        assert!(normalize("alt+t").is_err());
        assert!(normalize("ctrl+alt").is_err());
        assert!(normalize("").is_err());
    }

    #[test]
    fn normalizes_named_and_function_keys() {
        assert_eq!(normalize("win+shift+f5").unwrap(), "Shift+Super+F5");
        assert_eq!(normalize("Ctrl+Alt+Space").unwrap(), "Ctrl+Alt+Space");
        assert_eq!(normalize("alt+shift+esc").unwrap(), "Alt+Shift+Escape");
    }

    #[test]
    fn rejects_unknown_tokens_and_extra_keys() {
        assert!(normalize("ctrl+alt+banana").is_err());
        assert!(normalize("ctrl+alt+s+t").is_err());
        assert!(normalize("ctrl++alt+s").is_err());
    }

    #[test]
    fn collapses_duplicate_modifiers() {
        assert_eq!(normalize("ctrl+CTRL+alt+s").unwrap(), "Ctrl+Alt+S");
    }

    /// Task 15: startup registers the persisted settings, not the in-memory defaults.
    #[test]
    fn configured_accelerators_come_from_the_persisted_settings() {
        let mut settings = crate::settings::Settings::default();
        settings.capture_hotkey = "Ctrl+Shift+F9".to_string();
        settings.toggle_hotkey = "Ctrl+Alt+Shift+T".to_string();
        settings.edit_latest_hotkey = "Ctrl+Shift+E".to_string();
        settings.search_hotkey = "Ctrl+Shift+F".to_string();
        assert_eq!(
            configured_accelerators(&settings),
            [
                (Target::Capture, "Ctrl+Shift+F9".to_string()),
                (Target::Toggle, "Ctrl+Alt+Shift+T".to_string()),
                (Target::EditLatest, "Ctrl+Shift+E".to_string()),
                (Target::Search, "Ctrl+Shift+F".to_string()),
            ]
        );
        // The defaults remain the fallback source for an unmanaged/first-run state.
        let defaults = crate::settings::Settings::default();
        assert_eq!(
            configured_accelerators(&defaults),
            [
                (Target::Capture, DEFAULT_CAPTURE.to_string()),
                (Target::Toggle, DEFAULT_TOGGLE.to_string()),
                (Target::EditLatest, DEFAULT_EDIT_LATEST.to_string()),
                (Target::Search, DEFAULT_SEARCH.to_string()),
            ]
        );
        assert_eq!(Target::Capture.default_accelerator(), DEFAULT_CAPTURE);
        assert_eq!(Target::Toggle.default_accelerator(), DEFAULT_TOGGLE);
        assert_eq!(Target::EditLatest.default_accelerator(), DEFAULT_EDIT_LATEST);
        assert_eq!(Target::Search.default_accelerator(), DEFAULT_SEARCH);
    }

    /// W1.8/W2.3: every target uses the same wire names and `set_hotkey` parsing.
    #[test]
    fn targets_round_trip_through_the_wire_names() {
        for target in [
            Target::Capture,
            Target::Toggle,
            Target::EditLatest,
            Target::Search,
        ] {
            assert_eq!(Target::parse(target.as_str()), Some(target));
        }
        assert_eq!(Target::EditLatest.as_str(), "editLatest");
        assert_eq!(Target::Search.as_str(), "search");
        assert_eq!(Target::parse("edit_latest"), None);
        assert_eq!(Target::parse("EditLatest"), None);
        assert_eq!(Target::parse("Search"), None);
        assert_eq!(Target::parse(""), None);
    }

    /// The W1.7 startup dialog lists every failed shortcut (with its own label) and the tray hint,
    /// in the deterministic target order.
    #[test]
    fn failure_dialog_body_names_each_failed_shortcut() {
        let strings = i18n::tray_strings(i18n::Lang::En);
        let failures = vec![
            HotkeyFailure {
                target: Target::Capture,
                accelerator: "Ctrl+Alt+S".to_string(),
                message: "already registered".to_string(),
            },
            HotkeyFailure {
                target: Target::EditLatest,
                accelerator: "Ctrl+Alt+E".to_string(),
                message: "access denied".to_string(),
            },
            HotkeyFailure {
                target: Target::Search,
                accelerator: "Ctrl+Alt+F".to_string(),
                message: "conflict".to_string(),
            },
        ];
        let body = failure_dialog_body(strings, &failures);
        assert!(body.starts_with(strings.hotkey_error_intro), "got {body:?}");
        assert!(body.contains("Capture (Ctrl+Alt+S): already registered"), "got {body:?}");
        assert!(
            body.contains("Edit latest (Ctrl+Alt+E): access denied"),
            "got {body:?}"
        );
        assert!(body.contains("Search (Ctrl+Alt+F): conflict"), "got {body:?}");
        assert!(body.ends_with(strings.hotkey_error_hint), "got {body:?}");
        // No failures means no dialog at all (the caller skips the box before building the body).
        assert!(Hotkeys::default().failures().is_empty());
    }

    /// M15: saving the persisted accelerator again must still re-attempt registration, because a
    /// startup conflict (another application owns the key) leaves it unregistered until a retry.
    #[test]
    fn a_repeated_accelerator_is_a_registration_retry() {
        assert!(is_registration_retry("Ctrl+Alt+T", "Ctrl+Alt+T"));
        // The shortcut window sends the canonical form, but the persisted value may be hand-edited
        // or carry another case/order; both sides are normalized before comparing.
        assert!(is_registration_retry("ctrl+alt+t", "Ctrl+Alt+T"));
        assert!(is_registration_retry("Ctrl+Alt+Shift+F9", "shift+alt+ctrl+f9"));
        assert!(!is_registration_retry("Ctrl+Alt+T", "Ctrl+Alt+S"));
        assert!(!is_registration_retry("Ctrl+Alt+T", "Ctrl+Alt+Shift+T"));
        // An invalid request is not a retry: `set_setting` must report the validation error.
        assert!(!is_registration_retry("Ctrl+Alt+T", "T"));
        assert!(!is_registration_retry("Ctrl+Alt+T", ""));
        // An unparsable persisted value (hand-edited file) falls back to a real change attempt.
        assert!(!is_registration_retry("banana", "banana"));
    }

    /// The tray hint shows capture first, then toggle, edit latest, search; a later success clears
    /// its own failure only.
    #[test]
    fn recorded_failures_are_ordered_and_cleared_per_target() {
        let hotkeys = Hotkeys::default();
        hotkeys.record_failure(HotkeyFailure {
            target: Target::Search,
            accelerator: "Ctrl+Alt+F".to_string(),
            message: "search busy".to_string(),
        });
        hotkeys.record_failure(HotkeyFailure {
            target: Target::Toggle,
            accelerator: "Ctrl+Alt+T".to_string(),
            message: "toggle busy".to_string(),
        });
        hotkeys.record_failure(HotkeyFailure {
            target: Target::EditLatest,
            accelerator: "Ctrl+Alt+E".to_string(),
            message: "edit busy".to_string(),
        });
        hotkeys.record_failure(HotkeyFailure {
            target: Target::Capture,
            accelerator: "Ctrl+Alt+S".to_string(),
            message: "capture busy".to_string(),
        });
        let failures = hotkeys.failures();
        assert_eq!(failures.len(), 4);
        assert_eq!(failures[0].target, Target::Capture);
        assert_eq!(failures[1].target, Target::Toggle);
        assert_eq!(failures[2].target, Target::EditLatest);
        assert_eq!(failures[3].target, Target::Search);

        hotkeys.clear_failure(Target::Capture);
        let failures = hotkeys.failures();
        assert_eq!(failures.len(), 3);
        assert_eq!(failures[0].target, Target::Toggle);
        assert_eq!(failures[1].target, Target::EditLatest);
        assert_eq!(failures[2].target, Target::Search);
    }
}
