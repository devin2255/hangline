//! Windows "Open at login" registration: quoting, exact-path reconciliation and Task Manager state.
//!
//! `tauri-plugin-autostart` delegates to `auto-launch 0.6.0`, whose Windows backend writes
//! `format!("{} {}", app_path, args.join(" "))` into the `Run` key **without quotes**. A path with a
//! space (Program Files, a portable folder, `%TEMP%\Hangline Test`) therefore does not launch at
//! logon and is the classic unquoted-path hijack: Windows tries `C:\Program.exe`,
//! `C:\Program Files\...` prefixes in order. This module takes ownership of the value after the
//! plugin creates it and rewrites it as a properly quoted, argument-safe command line.
//!
//! It also owns the two registration states the plugin hides behind one boolean:
//! - a value that exists but points at a *moved* executable is stale and must be repaired;
//! - a `StartupApproved\Run` marker with the disabled state byte (`0x03`) is the user's Task Manager
//!   choice and must never be silently overridden by a startup reconcile.
//!
//! The command-line parser is `CommandLineToArgvW` itself, so the tests verify the value exactly as
//! Windows would parse it, not with a hand-rolled approximation.

use std::ffi::c_void;
use std::io;
use std::path::{Path, PathBuf};

use tauri::{AppHandle, Manager};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{
    LocalFree, ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, ERROR_SUCCESS, HLOCAL,
};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_QUERY_VALUE, KEY_SET_VALUE,
    REG_BINARY, REG_EXPAND_SZ, REG_OPTION_NON_VOLATILE, REG_SZ, REG_VALUE_TYPE,
};
use windows::Win32::UI::Shell::CommandLineToArgvW;

use crate::app_state::AppState;

/// The value name the plugin uses: `app.package_info().name` (Tauri `productName`, "Hangline").
/// Production code reads it from the live `AppHandle`; the tests use a unique throw-away name.
const RUN_KEY: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Run";
const STARTUP_APPROVED_KEY: &str =
    r"SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";

/// The two hives `auto-launch` reads/writes; a normal (non-elevated) run uses `HKCU`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Hive {
    CurrentUser,
    LocalMachine,
}

impl Hive {
    fn root(self) -> HKEY {
        match self {
            Self::CurrentUser => HKEY_CURRENT_USER,
            Self::LocalMachine => HKEY_LOCAL_MACHINE,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::CurrentUser => "HKCU",
            Self::LocalMachine => "HKLM",
        }
    }
}

/// One existing `Run` value (only the command text matters for reconciliation).
#[derive(Clone, Debug, PartialEq, Eq)]
struct RunValue {
    hive: Hive,
    command: String,
}

/// The real login-item state, independent of the persisted `startAtLogin` setting.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Registration {
    entries: Vec<RunValue>,
    /// At least one `StartupApproved\Run` marker is explicitly disabled (user choice in Task
    /// Manager / Settings > Startup).
    task_manager_disabled: bool,
}

impl Registration {
    /// A `Run` value exists in either hive (a manual registration counts).
    pub(crate) fn registered(&self) -> bool {
        !self.entries.is_empty()
    }

    /// The user disabled the item in Task Manager; startup reconcile must leave it disabled.
    pub(crate) fn task_manager_disabled(&self) -> bool {
        self.task_manager_disabled
    }

    /// Every existing value already is the canonical quoted command line for `exe` (arguments
    /// preserved). A moved executable, an unquoted spaced path or a changed quoting style is stale.
    fn needs_repair(&self, exe: &Path) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.command != normalized_command_for(&entry.command, exe))
    }
}

/// What startup reconciliation must do for the current setting and real registration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReconcileAction {
    /// Setting and registration already agree.
    None,
    /// Setting on, no `Run` value: register through the plugin (which normalizes the value).
    Enable,
    /// Setting on, a `Run` value exists but is stale/unquoted: rewrite it in place.
    Repair,
    /// Setting on but the user disabled the item in Task Manager: leave it disabled.
    LeaveTaskManagerDisabled,
    /// Setting off, a manual `Run` value exists: keep it (the tray shows the real state).
    KeepManual,
}

/// What [`normalize_registration`] changed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct NormalizeOutcome {
    /// Hives whose value was rewritten to the canonical quoted command line.
    pub(crate) rewritten: Vec<Hive>,
    /// No `Run` value existed; the canonical value was created in `HKCU`.
    pub(crate) created: bool,
}

// ---------------------------------------------------------------------------------------------
// Command-line quoting and parsing (the exact rules `CommandLineToArgvW` applies).
// ---------------------------------------------------------------------------------------------

/// Quotes one argument using the inverse of the `CommandLineToArgvW` rules: an argument is only
/// quoted when it needs to be; backslashes are doubled before a quote and at the end of a quoted
/// argument.
fn quote_arg(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '"']) {
        return arg.to_string();
    }
    let mut quoted = String::with_capacity(arg.len() + 2);
    quoted.push('"');
    let mut backslashes = 0usize;
    for character in arg.chars() {
        match character {
            '\\' => backslashes += 1,
            '"' => {
                // 2n + 1 backslashes produce n literal backslashes and one escaped quote.
                quoted.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                quoted.push('"');
                backslashes = 0;
            }
            _ => {
                quoted.extend(std::iter::repeat_n('\\', backslashes));
                quoted.push(character);
                backslashes = 0;
            }
        }
    }
    // 2n backslashes before the closing quote produce n literal backslashes.
    quoted.extend(std::iter::repeat_n('\\', backslashes * 2));
    quoted.push('"');
    quoted
}

/// Quotes the executable and every argument into one Windows command line.
///
/// The executable is always quoted, even without spaces: that is the whole point of owning the
/// `Run` value (an unquoted path is ambiguous and can be hijacked by an earlier executable).
pub(crate) fn command_line(exe: &Path, args: &[String]) -> String {
    let mut line = format!("\"{}\"", exe.display());
    for arg in args {
        line.push(' ');
        line.push_str(&quote_arg(arg));
    }
    line
}

/// Parses a command line exactly like Windows (`CommandLineToArgvW`).
pub(crate) fn parse_command_line(command: &str) -> Vec<String> {
    if command.trim().is_empty() {
        return Vec::new();
    }
    let wide: Vec<u16> = command.encode_utf16().chain(std::iter::once(0)).collect();
    let mut count = 0i32;
    // SAFETY: `wide` is a NUL-terminated UTF-16 buffer that outlives the call. On success Windows
    // allocates the argument vector with LocalAlloc; it is freed below with LocalFree.
    let argv = unsafe { CommandLineToArgvW(PCWSTR(wide.as_ptr()), &mut count) };
    if argv.is_null() {
        return Vec::new();
    }
    let mut parsed = Vec::with_capacity(count.max(0) as usize);
    for index in 0..count.max(0) as isize {
        let pointer = unsafe { (*argv.offset(index)).0 };
        if pointer.is_null() {
            parsed.push(String::new());
            continue;
        }
        let mut len = 0usize;
        while unsafe { *pointer.add(len) } != 0 {
            len += 1;
        }
        // SAFETY: the pointer is a NUL-terminated string owned by the vector just allocated.
        let units = unsafe { std::slice::from_raw_parts(pointer, len) };
        parsed.push(String::from_utf16_lossy(units));
    }
    // SAFETY: `argv` came from CommandLineToArgvW/LocalAlloc and is no longer used.
    unsafe { LocalFree(Some(HLOCAL(argv as *mut c_void))) };
    parsed
}

/// Compares two Windows paths case-insensitively, ignoring a trailing separator.
fn same_path(left: &str, right: &Path) -> bool {
    fn normalize(path: &str) -> String {
        path.trim().trim_end_matches(['\\', '/']).to_string()
    }
    normalize(left).eq_ignore_ascii_case(&normalize(&right.to_string_lossy()))
}

/// Whether the command's executable is *exactly* `exe` (not a prefix, not a partial token).
///
/// This is deliberately an exact parsed-path comparison. A prefix comparison (`startsWith`) would
/// accept `hangline.exe.bak` or `hangline.exe.tmp` and, for a spaced path, the unquoted value would
/// parse into several arguments and never match the executable.
pub(crate) fn run_value_targets_exe(command: &str, exe: &Path) -> bool {
    if command.trim().is_empty() {
        return false;
    }
    let argv = parse_command_line(command);
    match argv.first() {
        Some(first) => !first.is_empty() && same_path(first, exe),
        None => false,
    }
}

/// The arguments of an existing `Run` command, preserving them across a path repair.
///
/// A quoted command is parsed by Windows rules. For the unquoted form the plugin writes, the
/// executable boundary is the first `.exe` suffix: everything after it are the user's arguments.
/// (Blindly parsing the unquoted form would turn `C:\Program Files\...` into bogus arguments.)
fn existing_args(command: &str) -> Vec<String> {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    if trimmed.starts_with('"') {
        let mut argv = parse_command_line(trimmed);
        if !argv.is_empty() {
            argv.remove(0);
        }
        return argv;
    }
    let lower = trimmed.to_ascii_lowercase();
    if let Some(index) = lower.find(".exe") {
        let end = index + 4;
        let executable = &trimmed[..end];
        let looks_like_path =
            executable.contains('\\') || executable.contains('/') || executable.contains(':');
        if looks_like_path {
            let rest = trimmed[end..].trim_start();
            if rest.is_empty() {
                return Vec::new();
            }
            return parse_command_line(rest);
        }
    }
    Vec::new()
}

/// The canonical command line for `command`'s arguments with `exe` as the executable.
fn normalized_command_for(command: &str, exe: &Path) -> String {
    command_line(exe, &existing_args(command))
}

/// Rewrites every existing `Run` value as a quoted command line for `exe` (arguments preserved).
///
/// Called after the plugin created the value (enable) and by startup reconciliation for stale
/// values. If no value is visible at all, the canonical value is created in `HKCU` so a plugin
/// success can never leave an unquoted or missing registration behind.
pub(crate) fn normalize_registration(app_name: &str, exe: &Path) -> io::Result<NormalizeOutcome> {
    let mut outcome = NormalizeOutcome::default();
    let mut found = false;
    for hive in [Hive::CurrentUser, Hive::LocalMachine] {
        let Some(command) = read_run_value(hive, app_name)? else {
            continue;
        };
        found = true;
        let normalized = normalized_command_for(&command, exe);
        if normalized != command {
            write_registry_string(hive, RUN_KEY, app_name, &normalized)?;
            outcome.rewritten.push(hive);
        }
    }
    if !found {
        write_registry_string(
            Hive::CurrentUser,
            RUN_KEY,
            app_name,
            &command_line(exe, &[]),
        )?;
        outcome.created = true;
    }
    Ok(outcome)
}

/// Reads the real registration (both hives' `Run` and `StartupApproved` values).
pub(crate) fn read_registration(app_name: &str) -> io::Result<Registration> {
    let mut entries = Vec::new();
    for hive in [Hive::CurrentUser, Hive::LocalMachine] {
        if let Some(command) = read_run_value(hive, app_name)? {
            entries.push(RunValue { hive, command });
        }
    }
    let mut task_manager_disabled = false;
    for hive in [Hive::CurrentUser, Hive::LocalMachine] {
        if let Some(bytes) = read_startup_approved(hive, app_name)? {
            if startup_approved_disabled(&bytes) {
                task_manager_disabled = true;
            }
        }
    }
    Ok(Registration {
        entries,
        task_manager_disabled,
    })
}

/// `StartupApproved\Run` state byte semantics: `0x02` is enabled, `0x03` is disabled. Windows writes
/// a timestamp after the state byte when the user disables an item, but some entries (Docker
/// Desktop on this machine) have an all-zero tail — which is why `auto-launch`'s trailing-eight-
/// bytes check misreads them as enabled. Any state we do not positively recognize as enabled is
/// treated as disabled: reconcile must never override a user choice it cannot prove is enabled.
fn startup_approved_disabled(bytes: &[u8]) -> bool {
    !matches!(bytes.first(), Some(0x02))
}

/// The pure reconcile decision, shared by startup and the tests.
pub(crate) fn reconcile_action(
    start_at_login: bool,
    registration: &Registration,
    exe: &Path,
) -> ReconcileAction {
    if start_at_login {
        if registration.task_manager_disabled() {
            return ReconcileAction::LeaveTaskManagerDisabled;
        }
        if !registration.registered() {
            return ReconcileAction::Enable;
        }
        if registration.needs_repair(exe) {
            return ReconcileAction::Repair;
        }
        ReconcileAction::None
    } else if registration.registered() {
        ReconcileAction::KeepManual
    } else {
        ReconcileAction::None
    }
}

// ---------------------------------------------------------------------------------------------
// Application integration (plugin calls, tray state and the startup reconcile).
// ---------------------------------------------------------------------------------------------

/// The value name the plugin uses: `app.package_info().name` (Tauri `productName`, "Hangline").
fn login_item_name(app: &AppHandle) -> String {
    app.package_info().name.clone()
}

fn current_exe() -> Result<PathBuf, String> {
    std::env::current_exe().map_err(|error| format!("cannot resolve the current executable: {error}"))
}

/// The real "Open at login" switch for the tray: a registration for the exact current executable
/// that the user has not disabled in Task Manager. `None` when the registry cannot be read (the
/// tray then falls back to the persisted setting).
pub(crate) fn effective_enabled(app: &AppHandle) -> Option<bool> {
    let exe = std::env::current_exe().ok()?;
    let registration = read_registration(&login_item_name(app)).ok()?;
    Some(
        !registration.task_manager_disabled()
            && registration
                .entries
                .iter()
                .any(|entry| run_value_targets_exe(&entry.command, &exe)),
    )
}

/// Registers the login item through the plugin, then normalizes the value to a quoted command line.
///
/// If the value cannot be rewritten the plugin registration is rolled back: a stale unquoted value
/// must not be advertised as a successful enable (the setting is rolled back by the caller too).
pub(crate) fn enable(app: &AppHandle) -> Result<(), String> {
    use tauri_plugin_autostart::ManagerExt;

    app.autolaunch()
        .enable()
        .map_err(|error| format!("cannot create the login item: {error}"))?;
    let exe = current_exe()?;
    match normalize_registration(&login_item_name(app), &exe) {
        Ok(outcome) => {
            log::debug!(
                "autostart: normalized the login item to a quoted command line (rewritten {:?}, created {})",
                outcome.rewritten,
                outcome.created
            );
            Ok(())
        }
        Err(error) => {
            let _ = app.autolaunch().disable();
            Err(format!("cannot quote the login-item command line: {error}"))
        }
    }
}

/// Unregisters the login item through the plugin and verifies that no `Run` value is left behind.
pub(crate) fn disable(app: &AppHandle) -> Result<(), String> {
    use tauri_plugin_autostart::ManagerExt;

    app.autolaunch()
        .disable()
        .map_err(|error| format!("cannot remove the login item: {error}"))?;
    let name = login_item_name(app);
    for hive in [Hive::CurrentUser, Hive::LocalMachine] {
        match read_run_value(hive, &name) {
            Ok(Some(_)) => {
                if let Err(error) = delete_registry_value(hive, RUN_KEY, &name) {
                    log::warn!(
                        "autostart: the login item is still present in {} after disabling it ({error}); \
                         the next reconcile will retry",
                        hive.label()
                    );
                }
            }
            Ok(None) => {}
            Err(error) => log::warn!(
                "autostart: cannot verify the {} login item after disabling it: {error}",
                hive.label()
            ),
        }
    }
    Ok(())
}

/// Re-asserts the persisted "Open at login" intent against the *real* registration.
///
/// - setting on + no value: register (e.g. the value was deleted or a previous enable failed);
/// - setting on + a stale value (moved executable or unquoted path): repair it in place;
/// - setting on + Task Manager disabled: leave the user's choice alone;
/// - setting off + a manual value: keep it (the tray shows the real state).
pub(crate) fn reconcile(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let settings = state.snapshot().settings;
    let exe = match current_exe() {
        Ok(exe) => exe,
        Err(error) => {
            log::warn!("autostart: {error}");
            return;
        }
    };
    let app_name = login_item_name(app);
    let registration = match read_registration(&app_name) {
        Ok(registration) => registration,
        Err(error) => {
            log::warn!("autostart: cannot read the login-item registration: {error}");
            return;
        }
    };
    match reconcile_action(settings.start_at_login, &registration, &exe) {
        ReconcileAction::None => {}
        ReconcileAction::Enable => {
            log::info!(
                "autostart: startAtLogin is on but the login item is missing; re-registering"
            );
            if let Err(error) = enable(app) {
                log::warn!("autostart: cannot re-register the login item: {error}");
            }
        }
        ReconcileAction::Repair => {
            log::info!(
                "autostart: the login item is stale (wrong executable or unquoted path); \
                 repairing it for {}",
                exe.display()
            );
            match normalize_registration(&app_name, &exe) {
                Ok(outcome) => log::info!(
                    "autostart: repaired the login item (rewritten {:?}, created {})",
                    outcome.rewritten,
                    outcome.created
                ),
                Err(error) => log::warn!("autostart: cannot repair the login item: {error}"),
            }
        }
        ReconcileAction::LeaveTaskManagerDisabled => {
            log::info!(
                "autostart: startAtLogin is on but Task Manager has the login item disabled; \
                 leaving the user's Task Manager choice alone (the tray shows the real state)"
            );
        }
        ReconcileAction::KeepManual => {
            log::info!(
                "autostart: a login item is registered but startAtLogin is off; keeping the \
                 manual registration (the tray shows the real state)"
            );
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Registry plumbing (thin wrappers over the Win32 registry API; no `reg.exe` process spawns).
// ---------------------------------------------------------------------------------------------

fn to_wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn status_error(status: windows::Win32::Foundation::WIN32_ERROR) -> io::Error {
    io::Error::from_raw_os_error(status.0 as i32)
}

fn open_key(hive: Hive, key_path: &str, access: windows::Win32::System::Registry::REG_SAM_FLAGS) -> io::Result<Option<HKEY>> {
    let path = to_wide(key_path);
    let mut key = HKEY::default();
    let status = unsafe {
        RegOpenKeyExW(
            hive.root(),
            PCWSTR(path.as_ptr()),
            None,
            access,
            &mut key,
        )
    };
    if status == ERROR_FILE_NOT_FOUND || status == ERROR_PATH_NOT_FOUND {
        return Ok(None);
    }
    if status != ERROR_SUCCESS {
        return Err(status_error(status));
    }
    Ok(Some(key))
}

fn read_registry_value(
    hive: Hive,
    key_path: &str,
    value_name: &str,
) -> io::Result<Option<(REG_VALUE_TYPE, Vec<u8>)>> {
    let Some(key) = open_key(hive, key_path, KEY_QUERY_VALUE)? else {
        return Ok(None);
    };
    let name = to_wide(value_name);
    let mut kind = REG_VALUE_TYPE(0);
    let mut size = 0u32;
    let status = unsafe {
        RegQueryValueExW(
            key,
            PCWSTR(name.as_ptr()),
            None,
            Some(&mut kind),
            None,
            Some(&mut size),
        )
    };
    if status == ERROR_FILE_NOT_FOUND {
        unsafe { let _ = RegCloseKey(key); };
        return Ok(None);
    }
    if status != ERROR_SUCCESS {
        unsafe { let _ = RegCloseKey(key); };
        return Err(status_error(status));
    }
    let mut data = vec![0u8; size as usize];
    let mut written = size;
    let status = unsafe {
        RegQueryValueExW(
            key,
            PCWSTR(name.as_ptr()),
            None,
            Some(&mut kind),
            Some(data.as_mut_ptr()),
            Some(&mut written),
        )
    };
    unsafe { let _ = RegCloseKey(key); };
    if status != ERROR_SUCCESS {
        return Err(status_error(status));
    }
    data.truncate(written as usize);
    Ok(Some((kind, data)))
}

fn read_registry_string(
    hive: Hive,
    key_path: &str,
    value_name: &str,
) -> io::Result<Option<String>> {
    let Some((kind, data)) = read_registry_value(hive, key_path, value_name)? else {
        return Ok(None);
    };
    if kind != REG_SZ && kind != REG_EXPAND_SZ {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "the login-item value has registry type {} (expected REG_SZ)",
                kind.0
            ),
        ));
    }
    if data.len() % 2 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "the login-item value has an odd number of UTF-16 bytes",
        ));
    }
    let units: Vec<u16> = data
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    let len = units.iter().position(|&unit| unit == 0).unwrap_or(units.len());
    Ok(Some(String::from_utf16_lossy(&units[..len])))
}

/// The `Run` command for `app_name` in `hive`, if the value exists.
fn read_run_value(hive: Hive, app_name: &str) -> io::Result<Option<String>> {
    read_registry_string(hive, RUN_KEY, app_name)
}

/// The raw `StartupApproved\Run` marker for `app_name` in `hive`, if the value exists.
fn read_startup_approved(hive: Hive, app_name: &str) -> io::Result<Option<Vec<u8>>> {
    let Some((kind, data)) = read_registry_value(hive, STARTUP_APPROVED_KEY, app_name)? else {
        return Ok(None);
    };
    if kind != REG_BINARY {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "the StartupApproved marker has registry type {} (expected REG_BINARY)",
                kind.0
            ),
        ));
    }
    Ok(Some(data))
}

fn write_registry_value(
    hive: Hive,
    key_path: &str,
    value_name: &str,
    kind: REG_VALUE_TYPE,
    data: &[u8],
) -> io::Result<()> {
    let path = to_wide(key_path);
    let mut key = HKEY::default();
    let status = unsafe {
        RegCreateKeyExW(
            hive.root(),
            PCWSTR(path.as_ptr()),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE | KEY_QUERY_VALUE,
            None,
            &mut key,
            None,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(status_error(status));
    }
    let name = to_wide(value_name);
    let status = unsafe { RegSetValueExW(key, PCWSTR(name.as_ptr()), None, kind, Some(data)) };
    unsafe { let _ = RegCloseKey(key); };
    if status != ERROR_SUCCESS {
        return Err(status_error(status));
    }
    Ok(())
}

fn write_registry_string(
    hive: Hive,
    key_path: &str,
    value_name: &str,
    value: &str,
) -> io::Result<()> {
    let mut data: Vec<u8> = value
        .encode_utf16()
        .flat_map(|unit| unit.to_le_bytes())
        .collect();
    data.extend_from_slice(&[0, 0]);
    write_registry_value(hive, key_path, value_name, REG_SZ, &data)
}

#[cfg(test)]
fn write_registry_bytes(
    hive: Hive,
    key_path: &str,
    value_name: &str,
    value: &[u8],
) -> io::Result<()> {
    write_registry_value(hive, key_path, value_name, REG_BINARY, value)
}

fn delete_registry_value(hive: Hive, key_path: &str, value_name: &str) -> io::Result<()> {
    let Some(key) = open_key(hive, key_path, KEY_SET_VALUE)? else {
        return Ok(());
    };
    let name = to_wide(value_name);
    let status = unsafe { RegDeleteValueW(key, PCWSTR(name.as_ptr())) };
    unsafe { let _ = RegCloseKey(key); };
    if status == ERROR_FILE_NOT_FOUND || status == ERROR_SUCCESS {
        return Ok(());
    }
    Err(status_error(status))
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;
    use std::path::PathBuf;

    /// A unique value name per test plus the spaced temporary folder the executable is copied into.
    /// Dropping the scope removes every registry value and the folder, even after a failed assert.
    struct TestScope {
        app_name: String,
        dir: PathBuf,
    }

    impl TestScope {
        fn new(tag: &str) -> Self {
            let app_name = format!("HanglineP4Test-{}-{tag}", std::process::id());
            let dir = std::env::temp_dir()
                .join("Hangline Test")
                .join(format!("{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).expect("create the spaced test directory");
            cleanup_values(&app_name);
            Self { app_name, dir }
        }

        /// `%TEMP%\Hangline Test\<tag>-<pid>\hangline.exe`: a path with spaces, like Program Files.
        fn spaced_exe(&self) -> PathBuf {
            let exe = self.dir.join("hangline.exe");
            fs::copy(std::env::current_exe().expect("resolve the test exe"), &exe)
                .expect("copy the test exe into the spaced directory");
            exe
        }
    }

    impl Drop for TestScope {
        fn drop(&mut self) {
            cleanup_values(&self.app_name);
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    fn cleanup_values(app_name: &str) {
        for hive in [Hive::CurrentUser, Hive::LocalMachine] {
            let _ = delete_registry_value(hive, RUN_KEY, app_name);
            let _ = delete_registry_value(hive, STARTUP_APPROVED_KEY, app_name);
        }
    }

    /// The exact command Windows would parse: one argument, the spaced executable path.
    #[test]
    fn a_spaced_executable_is_quoted_as_one_argument() {
        let exe = PathBuf::from(r"C:\Users\some one\AppData\Local\Programs\Hangline\hangline.exe");
        let command = command_line(&exe, &[]);

        assert_eq!(command, format!("\"{}\"", exe.display()));
        let argv = parse_command_line(&command);
        assert_eq!(argv, vec![exe.display().to_string()]);
        assert!(run_value_targets_exe(&command, &exe));
    }

    /// Quoting rules for arguments (spaces, embedded quotes, backslashes and empty arguments).
    #[test]
    fn arguments_are_quoted_with_windows_rules() {
        let exe = PathBuf::from(r"C:\Program Files\Hangline\hangline.exe");
        let args = vec![
            "--profile".to_string(),
            r"C:\My Shots".to_string(),
            "two words".to_string(),
            String::new(),
            "say \"hi\"".to_string(),
            r"C:\Some Dir\".to_string(),
        ];
        let command = command_line(&exe, &args);

        assert_eq!(
            command,
            r#""C:\Program Files\Hangline\hangline.exe" --profile "C:\My Shots" "two words" "" "say \"hi\"" "C:\Some Dir\\""#
        );
        // CommandLineToArgvW agrees with the quoting: the executable plus the exact arguments.
        let argv = parse_command_line(&command);
        assert_eq!(argv.len(), args.len() + 1);
        assert_eq!(argv[0], exe.display().to_string());
        assert_eq!(&argv[1..], args.as_slice());
        assert!(run_value_targets_exe(&command, &exe));
    }

    /// The parser must expose the unquoted-path bug: splitting at the first space cannot equal the
    /// spaced executable, so a prefix comparison is not enough.
    #[test]
    fn the_parser_follows_windows_rules_for_unquoted_spaced_paths() {
        let exe = PathBuf::from(r"C:\Program Files\Hangline\hangline.exe");
        let unquoted = r"C:\Program Files\Hangline\hangline.exe";
        let argv = parse_command_line(unquoted);
        assert_eq!(argv[0], r"C:\Program");
        assert_ne!(argv[0], exe.display().to_string());
        assert!(!run_value_targets_exe(unquoted, &exe));

        // A quoted path is a single token even with spaces and a trailing argument.
        let quoted = r#""C:\Program Files\Hangline\hangline.exe" --flag"#;
        assert_eq!(
            parse_command_line(quoted),
            vec![exe.display().to_string(), "--flag".to_string()]
        );
        assert!(run_value_targets_exe(quoted, &exe));
    }

    /// Exact executable identity: a prefix (`hangline.exe.bak`), a longer name, or a different path
    /// must never count as the current executable. Windows paths compare case-insensitively.
    #[test]
    fn the_target_check_requires_the_exact_executable() {
        let exe = PathBuf::from(r"C:\Program Files\Hangline\hangline.exe");

        assert!(run_value_targets_exe(
            r#""C:\Program Files\Hangline\hangline.exe""#,
            &exe
        ));
        assert!(run_value_targets_exe(
            r#""c:\program files\hangline\HANGLINE.EXE" --flag"#,
            &exe
        ));
        assert!(
            !run_value_targets_exe(r#""C:\Program Files\Hangline\hangline.exe.bak""#, &exe),
            "a prefix match would accept a different executable"
        );
        assert!(
            run_value_targets_exe(r#""C:\Program Files\Hangline\hangline.exe" extra"#, &exe),
            "arguments after a properly quoted executable do not change its identity"
        );
        assert!(
            !run_value_targets_exe(r#""C:\Program Files\Hangline\hangline.exe.tmp""#, &exe),
            "a longer file name is a different executable"
        );
        assert!(
            !run_value_targets_exe(r#""C:\Other\hangline.exe""#, &exe),
            "a different directory is a different executable"
        );
        assert!(!run_value_targets_exe("", &exe), "an empty value has no target");
        assert!(!run_value_targets_exe("   ", &exe), "blank is not a target");
    }

    /// The plugin writes `"<exe> "` (no quotes, trailing space). Enabling must normalize the HKCU
    /// value to exactly one quoted argument pointing at the spaced executable.
    #[test]
    fn a_plugin_written_value_is_normalized_to_a_quoted_spaced_path() {
        let scope = TestScope::new("quoted-spaced");
        let exe = scope.spaced_exe();
        let plugin_value = format!("{} ", exe.display());
        write_registry_string(Hive::CurrentUser, RUN_KEY, &scope.app_name, &plugin_value)
            .expect("write the plugin-style value");

        let outcome = normalize_registration(&scope.app_name, &exe).expect("normalize");
        assert_eq!(outcome.rewritten, vec![Hive::CurrentUser]);
        assert!(!outcome.created);

        let value = read_registry_string(Hive::CurrentUser, RUN_KEY, &scope.app_name)
            .expect("read the normalized value")
            .expect("the value exists");
        assert_eq!(value, format!("\"{}\"", exe.display()));
        let argv = parse_command_line(&value);
        assert_eq!(
            argv,
            vec![exe.display().to_string()],
            "CommandLineToArgvW must see one executable argument"
        );
        assert!(run_value_targets_exe(&value, &exe));
    }

    /// A stale value left by a moved portable build (or an update) is repaired to the current exe,
    /// and user-provided arguments are preserved.
    #[test]
    fn a_stale_run_value_is_repaired_and_preserves_arguments() {
        let scope = TestScope::new("stale");
        let exe = scope.spaced_exe();
        let stale = r#"C:\Program Files\Old Hangline\hangline.exe --flag "two words""#;
        write_registry_string(Hive::CurrentUser, RUN_KEY, &scope.app_name, stale)
            .expect("write a stale value");

        let before = read_registration(&scope.app_name).expect("read stale registration");
        assert!(before.registered());
        assert_eq!(
            reconcile_action(true, &before, &exe),
            ReconcileAction::Repair,
            "a stale executable must be repaired, not reported as enabled"
        );

        let outcome = normalize_registration(&scope.app_name, &exe).expect("normalize");
        assert_eq!(outcome.rewritten, vec![Hive::CurrentUser]);
        let repaired = read_registry_string(Hive::CurrentUser, RUN_KEY, &scope.app_name)
            .expect("read the repaired value")
            .expect("the value exists");
        assert_eq!(
            repaired,
            format!("\"{}\" --flag \"two words\"", exe.display())
        );
        let argv = parse_command_line(&repaired);
        assert_eq!(argv[0], exe.display().to_string());
        assert_eq!(argv[1], "--flag");
        assert_eq!(argv[2], "two words");
        assert!(run_value_targets_exe(&repaired, &exe));

        let after = read_registration(&scope.app_name).expect("read repaired registration");
        assert_eq!(
            reconcile_action(true, &after, &exe),
            ReconcileAction::None,
            "a repaired registration is in sync"
        );
    }

    /// The actual plugin output for a spaced install is *unquoted*: the executable boundary must be
    /// recovered from the `.exe` suffix, not by treating every space-separated fragment as an
    /// argument (which would preserve bogus `Files\Old Hangline\hangline.exe` arguments).
    #[test]
    fn an_unquoted_spaced_stale_value_is_repaired_without_inventing_arguments() {
        let scope = TestScope::new("unquoted-stale");
        let exe = scope.spaced_exe();
        let plugin_style = r"C:\Program Files\Old Hangline\hangline.exe --flag";
        write_registry_string(Hive::CurrentUser, RUN_KEY, &scope.app_name, plugin_style)
            .expect("write an unquoted stale value");

        let outcome = normalize_registration(&scope.app_name, &exe).expect("normalize");
        assert_eq!(outcome.rewritten, vec![Hive::CurrentUser]);
        let repaired = read_registry_string(Hive::CurrentUser, RUN_KEY, &scope.app_name)
            .expect("read the repaired value")
            .expect("the value exists");
        assert_eq!(repaired, format!("\"{}\" --flag", exe.display()));
        assert_eq!(
            parse_command_line(&repaired),
            vec![exe.display().to_string(), "--flag".to_string()]
        );
    }

    /// A plugin success that is not visible as a value (e.g. a view/permission oddity) still leaves
    /// a canonical quoted HKCU value behind instead of reporting an enable with no login command.
    #[test]
    fn a_missing_value_is_created_as_a_quoted_command() {
        let scope = TestScope::new("create");
        let exe = scope.spaced_exe();

        let outcome = normalize_registration(&scope.app_name, &exe).expect("normalize");
        assert!(outcome.created);
        assert!(outcome.rewritten.is_empty());
        let value = read_registry_string(Hive::CurrentUser, RUN_KEY, &scope.app_name)
            .expect("read the created value")
            .expect("the value exists");
        assert_eq!(value, command_line(&exe, &[]));
        assert!(read_registration(&scope.app_name).expect("read").registered());
    }

    /// A `StartupApproved` marker with byte `0x03` is the user's Task Manager "disabled" choice.
    /// The auto-launch crate only looks at the trailing eight bytes (zero for some disabled
    /// markers, e.g. Docker Desktop on this machine), so it can report a disabled item as enabled.
    #[test]
    fn a_task_manager_disabled_marker_is_detected() {
        let scope = TestScope::new("tm-disabled");
        let exe = scope.spaced_exe();
        write_registry_string(
            Hive::CurrentUser,
            RUN_KEY,
            &scope.app_name,
            &command_line(&exe, &[]),
        )
        .expect("write the Run value");
        // Task Manager's disabled marker, both without and with the timestamp Windows writes.
        write_registry_bytes(
            Hive::CurrentUser,
            STARTUP_APPROVED_KEY,
            &scope.app_name,
            &[0x03, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        )
        .expect("write the zero-timestamp disabled marker");

        let disabled = read_registration(&scope.app_name).expect("read registration");
        assert!(disabled.registered());
        assert!(disabled.task_manager_disabled());
        assert_eq!(
            reconcile_action(true, &disabled, &exe),
            ReconcileAction::LeaveTaskManagerDisabled,
            "startup reconcile must not override the user's Task Manager choice"
        );

        write_registry_bytes(
            Hive::CurrentUser,
            STARTUP_APPROVED_KEY,
            &scope.app_name,
            &[0x03, 0, 0, 0, 0x6f, 0xa9, 0x0a, 0x18, 0xee, 0xd6, 0xdc, 0x01],
        )
        .expect("write the timestamped disabled marker");
        assert!(
            read_registration(&scope.app_name)
                .expect("read registration")
                .task_manager_disabled(),
            "a timestamped disabled marker is disabled too"
        );

        write_registry_bytes(
            Hive::CurrentUser,
            STARTUP_APPROVED_KEY,
            &scope.app_name,
            &[0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        )
        .expect("write the enabled marker");
        let enabled = read_registration(&scope.app_name).expect("read registration");
        assert!(!enabled.task_manager_disabled());
        assert_eq!(reconcile_action(true, &enabled, &exe), ReconcileAction::None);
    }

    /// A manual registration while `startAtLogin` is off is kept, not deleted by reconcile.
    #[test]
    fn a_manual_registration_is_kept_when_the_setting_is_off() {
        let scope = TestScope::new("manual");
        let exe = scope.spaced_exe();
        write_registry_string(
            Hive::CurrentUser,
            RUN_KEY,
            &scope.app_name,
            &command_line(&exe, &[]),
        )
        .expect("write the manual value");
        let manual = read_registration(&scope.app_name).expect("read registration");
        assert_eq!(
            reconcile_action(false, &manual, &exe),
            ReconcileAction::KeepManual
        );
    }

    /// The real registration requires the exact current executable and not a Task Manager disable.
    #[test]
    fn effective_enabled_requires_the_exact_current_executable() {
        let scope = TestScope::new("effective");
        let exe = scope.spaced_exe();
        write_registry_string(
            Hive::CurrentUser,
            RUN_KEY,
            &scope.app_name,
            r#""C:\Old Place\hangline.exe""#,
        )
        .expect("write a stale value");
        let stale = read_registration(&scope.app_name).expect("read registration");
        assert!(stale.registered());
        assert!(
            !stale
                .entries
                .iter()
                .any(|entry| run_value_targets_exe(&entry.command, &exe)),
            "a stale path is registered but not effective"
        );
        assert_eq!(reconcile_action(true, &stale, &exe), ReconcileAction::Repair);

        write_registry_string(
            Hive::CurrentUser,
            RUN_KEY,
            &scope.app_name,
            &command_line(&exe, &[]),
        )
        .expect("write the current value");
        let current = read_registration(&scope.app_name).expect("read registration");
        assert!(
            current
                .entries
                .iter()
                .any(|entry| run_value_targets_exe(&entry.command, &exe))
        );
        assert_eq!(reconcile_action(true, &current, &exe), ReconcileAction::None);
    }

    /// Missing registration + setting on means "register"; missing + setting off means "nothing".
    #[test]
    fn a_missing_registration_is_reported_as_missing() {
        let scope = TestScope::new("missing");
        let exe = scope.spaced_exe();
        let registration = read_registration(&scope.app_name).expect("read registration");
        assert!(!registration.registered());
        assert!(!registration.task_manager_disabled());
        assert_eq!(
            reconcile_action(true, &registration, &exe),
            ReconcileAction::Enable
        );
        assert_eq!(
            reconcile_action(false, &registration, &exe),
            ReconcileAction::None
        );
    }
}
