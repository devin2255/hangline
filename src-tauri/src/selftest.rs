//! `--selftest` field diagnostics for a Hangline install.
//!
//! `hangline.exe --selftest` runs a fixed list of probes against the machine the app is installed
//! on and exits immediately: no Tauri builder, no window, no tray, no WebView2 and no single-instance
//! lock, so the command also works when the app itself fails to start. `--selftest --json` prints
//! the same checks as one JSON object per line for scripts/support tickets.
//!
//! The module belongs to the **binary** crate: `main.rs` declares it with `#[path = "selftest.rs"]
//! mod selftest;`, so it cannot reuse the lib's private modules. Paths (`%LOCALAPPDATA%\Hangline`),
//! the `settings.json` / `state.json` shapes, the autostart Run value name and the WebView2 lookup are
//! therefore deliberately re-implemented here on top of `std` + the dependencies the binary already
//! links (`serde_json`, `windows`). No new crate is introduced and `Cargo.toml` stays untouched.
//!
//! Contract:
//! - one line per probe: `[PASS] name detail`, `[WARN] name detail`, `[FAIL] name detail`;
//! - WARN means "the app still runs, but check this" and never affects the exit code;
//! - the process exit code is the number of FAIL probes (0 = healthy);
//! - a panicking probe is reported as FAIL instead of aborting the remaining checks;
//! - stdout is best-effort: a release build uses the `windows` subsystem and usually has no console,
//!   so the same report is always appended to `%LOCALAPPDATA%\Hangline\logs\selftest.log` with a
//!   timestamp (that file is the evidence on a customer machine).
//!
//! The memory probe talks to `kernel32!K32GetProcessMemoryInfo` through a hand-written declaration:
//! `GetProcessMemoryInfo` lives in the crate's `Win32::System::ProcessStatus` feature, which this
//! crate does not enable. If that call ever fails at runtime the probe degrades to WARN and still
//! reports the process handle count (a `GetProcessHandleCount` probe from an enabled feature).

use std::ffi::OsStr;
use std::fs;
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};

use serde_json::{json, Value};

use windows::core::{BOOL, PCWSTR};
use windows::Win32::Foundation::{
    ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, ERROR_SUCCESS, HANDLE, SYSTEMTIME,
};
use windows::Win32::Graphics::Gdi::{GetDC, GetDeviceCaps, ReleaseDC, LOGPIXELSX};
use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
use windows::Win32::System::Registry::{
    RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE,
    REG_EXPAND_SZ, REG_SZ, REG_VALUE_TYPE,
};
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessHandleCount};
use windows::Win32::UI::HiDpi::{
    AreDpiAwarenessContextsEqual, GetDpiForSystem, GetThreadDpiAwarenessContext,
    DPI_AWARENESS_CONTEXT, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE,
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, DPI_AWARENESS_CONTEXT_SYSTEM_AWARE,
    DPI_AWARENESS_CONTEXT_UNAWARE, DPI_AWARENESS_CONTEXT_UNAWARE_GDISCALED,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CMONITORS, SM_CXSCREEN, SM_CXVIRTUALSCREEN, SM_CYSCREEN,
    SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

/// Minimum free space worth a warning: below this, captures/recordings will start failing soon.
const MIN_FREE_DISK_BYTES: u64 = 500 * 1024 * 1024;

/// Directory under the install roots that holds the WebView2 Evergreen runtime versions.
const WEBVIEW2_APP_SUBDIR: &str = r"Microsoft\EdgeWebView\Application";

/// `HKCU` key the Tauri autostart plugin writes the login item into.
const RUN_KEY: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Run";

/// The value name the autostart plugin uses (Tauri `productName`).
const AUTOSTART_VALUE: &str = "Hangline";

/// Where the user can get the WebView2 runtime when it is missing.
const WEBVIEW2_HINT: &str =
    "install the WebView2 Evergreen Runtime: https://developer.microsoft.com/microsoft-edge/webview2/";

/// How the binary was invoked. Created by [`parse_mode`] before any Tauri initialisation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// The normal application start: `hangline_lib::run()` runs exactly as before `--selftest`.
    Normal,
    /// Field diagnostics only: no Tauri, no window, no tray; exit code = FAIL count.
    SelfTest {
        /// `--json` was also present: emit one JSON object per line instead of the text report.
        json: bool,
    },
}

/// Parses the command line into the run mode.
///
/// Only the two exact flags are recognised (`--selftest`, `--json`); unknown arguments are ignored
/// so the normal path keeps its previous behaviour of never looking at the command line. `--json`
/// on its own does **not** start the selftest (it is only a modifier for `--selftest`).
pub fn parse_mode<I>(args: I) -> Mode
where
    I: IntoIterator,
    I::Item: AsRef<OsStr>,
{
    let mut selftest = false;
    let mut json = false;
    for arg in args {
        match arg.as_ref().to_str() {
            Some("--selftest") => selftest = true,
            Some("--json") => json = true,
            _ => {}
        }
    }
    if selftest {
        Mode::SelfTest { json }
    } else {
        Mode::Normal
    }
}

/// Outcome of one probe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Working as intended.
    Pass,
    /// Degraded or unexpected, but the app still runs; does not change the exit code.
    Warn,
    /// The app (or the feature under test) cannot work; each FAIL adds one to the exit code.
    Fail,
}

impl Status {
    /// Upper-case tag used by both output formats.
    fn as_str(self) -> &'static str {
        match self {
            Status::Pass => "PASS",
            Status::Warn => "WARN",
            Status::Fail => "FAIL",
        }
    }
}

/// One rendered diagnostic row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    /// Stable probe id (`data-dir`, `settings-json`, ...).
    name: &'static str,
    /// Probe outcome.
    status: Status,
    /// Human-readable explanation, including the observed value or the error.
    detail: String,
}

impl Check {
    fn new(name: &'static str, status: Status, detail: impl Into<String>) -> Self {
        Self {
            name,
            status,
            detail: detail.into(),
        }
    }
}

/// Aggregate counters for one run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    /// Number of PASS rows.
    pass: usize,
    /// Number of WARN rows.
    warn: usize,
    /// Number of FAIL rows.
    fail: usize,
}

impl Summary {
    /// Counts the rows of a finished probe run.
    fn from_checks(checks: &[Check]) -> Self {
        let mut summary = Summary::default();
        for check in checks {
            match check.status {
                Status::Pass => summary.pass += 1,
                Status::Warn => summary.warn += 1,
                Status::Fail => summary.fail += 1,
            }
        }
        summary
    }

    /// Process exit code: the FAIL count (WARN is informational only, like the task contract).
    ///
    /// Clamped to 255 because `ExitProcess` only exposes the low byte to `cmd`/`$LASTEXITCODE`;
    /// with 8–12 probes the clamp never triggers in practice.
    fn exit_code(self) -> i32 {
        self.fail.min(255) as i32
    }
}

/// A probe function: returns the status plus the one-line detail.
type Probe = fn() -> (Status, String);

/// The probe list, in report order. The names are the stable ids shown in `--json`.
static PROBES: &[(&str, Probe)] = &[
    ("data-dir", probe_data_dir),
    ("settings-json", probe_settings_json),
    ("state-json", probe_state_json),
    ("disk-free", probe_disk_free),
    ("dpi", probe_dpi),
    ("display", probe_display),
    ("webview2", probe_webview2),
    ("memory", probe_memory),
    ("autostart", probe_autostart),
    ("version", probe_version),
];

/// Runs every probe, isolating panics so one broken probe cannot hide the others.
pub fn run_checks() -> Vec<Check> {
    PROBES
        .iter()
        .map(|&(name, probe)| probe_into_check(name, probe))
        .collect()
}

/// Runs one probe and converts it to a [`Check`]; a panic becomes a FAIL row.
fn probe_into_check(name: &'static str, probe: Probe) -> Check {
    match std::panic::catch_unwind(probe) {
        Ok((status, detail)) => Check::new(name, status, detail),
        Err(_) => Check::new(
            name,
            Status::Fail,
            "probe panicked (internal selftest error; the app itself is unaffected)",
        ),
    }
}

/// Runs the selftest, prints the report, appends it to the log and returns the exit code.
///
/// Called by `main` only when `--selftest` was passed; the caller is responsible for
/// `std::process::exit` so this function stays unit-testable.
pub fn run(json_output: bool) -> i32 {
    let checks = run_checks();
    let summary = Summary::from_checks(&checks);
    let exit_code = summary.exit_code();
    let timestamp = local_timestamp();
    let report = if json_output {
        render_json(&checks, summary)
    } else {
        render_text(&checks, summary, &timestamp)
    };

    // A release build is a `windows` subsystem binary: stdout may not exist, so a failed write is
    // ignored on purpose (the log file below is the durable channel).
    let mut stdout = io::stdout().lock();
    let _ = stdout.write_all(report.as_bytes());
    let _ = stdout.flush();
    drop(stdout);

    if let Err(error) = append_log(&report, &timestamp, exit_code) {
        let mut stderr = io::stderr().lock();
        let _ = writeln!(
            stderr,
            "selftest: could not append to %LOCALAPPDATA%\\Hangline\\logs\\selftest.log: {error}"
        );
    }
    exit_code
}

/// Human-readable report: header, one line per probe, summary line.
fn render_text(checks: &[Check], summary: Summary, timestamp: &str) -> String {
    let mut report = String::new();
    report.push_str(&format!(
        "Hangline selftest v{}\n",
        env!("CARGO_PKG_VERSION")
    ));
    report.push_str(&format!("time: {timestamp}\n"));
    report.push_str(
        "note: --selftest does not initialise Tauri/windows/WebView2; the memory probe is the \
         bare-process baseline.\n",
    );
    report.push_str("----\n");
    for check in checks {
        report.push_str(&format!(
            "[{:<4}] {:<14} {}\n",
            check.status.as_str(),
            check.name,
            check.detail
        ));
    }
    report.push_str("----\n");
    report.push_str(&format!(
        "PASS {}  WARN {}  FAIL {}  exit code {}\n",
        summary.pass,
        summary.warn,
        summary.fail,
        summary.exit_code()
    ));
    report
}

/// Machine-readable report: one JSON object per probe plus a final `{"summary": ...}` line.
fn render_json(checks: &[Check], summary: Summary) -> String {
    let mut report = String::new();
    for check in checks {
        let line = json!({
            "name": check.name,
            "status": check.status.as_str(),
            "detail": check.detail,
        });
        report.push_str(&line.to_string());
        report.push('\n');
    }
    let summary_line = json!({
        "summary": {
            "pass": summary.pass,
            "warn": summary.warn,
            "fail": summary.fail,
            "exitCode": summary.exit_code(),
        }
    });
    report.push_str(&summary_line.to_string());
    report.push('\n');
    report
}

/// `%LOCALAPPDATA%\Hangline\logs\selftest.log`, the durable copy of a field run.
fn log_path() -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA").filter(|value| !value.is_empty())?;
    Some(
        PathBuf::from(base)
            .join("Hangline")
            .join("logs")
            .join("selftest.log"),
    )
}

/// Appends one timestamped report to the log (best effort; creates `logs\` if needed).
fn append_log(report: &str, timestamp: &str, exit_code: i32) -> io::Result<()> {
    let path = log_path().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "LOCALAPPDATA is not set; cannot locate the selftest log",
        )
    })?;
    append_log_to(&path, report, timestamp, exit_code)
}

/// [`append_log`] against an explicit path (unit-testable with a scratch directory).
fn append_log_to(path: &Path, report: &str, timestamp: &str, exit_code: i32) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(
        file,
        "===== Hangline selftest {timestamp} exit={exit_code} ====="
    )?;
    file.write_all(report.as_bytes())?;
    if !report.ends_with('\n') {
        file.write_all(b"\n")?;
    }
    file.flush()
}

/// Current local time, formatted for the report/log header.
fn local_timestamp() -> String {
    // SAFETY: GetLocalTime has no inputs and writes the SYSTEMTIME it returns.
    format_system_time(&unsafe { GetLocalTime() })
}

/// Formats a `SYSTEMTIME` as `yyyy-MM-dd HH:mm:ss.mmm (local)`.
fn format_system_time(time: &SYSTEMTIME) -> String {
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03} (local)",
        time.wYear,
        time.wMonth,
        time.wDay,
        time.wHour,
        time.wMinute,
        time.wSecond,
        time.wMilliseconds
    )
}

// ---------------------------------------------------------------------------------------------
// Probes
// ---------------------------------------------------------------------------------------------

/// `%LOCALAPPDATA%\Hangline`: exists/creatable, writable, read-back works, probe file removed.
fn probe_data_dir() -> (Status, String) {
    match data_dir_path() {
        Ok(dir) => check_data_dir_writable(&dir),
        Err(error) => (Status::Fail, error),
    }
}

/// Creates `dir` and verifies a probe file can be written, read back and removed there.
///
/// Takes the directory as an argument so the unit tests can exercise both the PASS and the FAIL
/// path against a scratch directory under `target/` (never the real `%LOCALAPPDATA%`).
fn check_data_dir_writable(dir: &Path) -> (Status, String) {
    if let Err(error) = fs::create_dir_all(dir) {
        return (
            Status::Fail,
            format!("cannot create {}: {error}", dir.display()),
        );
    }

    const PAYLOAD: &[u8] = b"hangline selftest\n";
    let probe_file = dir.join(format!(".selftest-probe-{}.tmp", std::process::id()));
    let write_result = (|| -> io::Result<()> {
        fs::write(&probe_file, PAYLOAD)?;
        let read_back = fs::read(&probe_file)?;
        if read_back.as_slice() != PAYLOAD {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "read-back mismatch",
            ));
        }
        Ok(())
    })();
    let cleanup = fs::remove_file(&probe_file);
    if let Err(error) = write_result {
        let _ = cleanup;
        return (
            Status::Fail,
            format!(
                "{} is not writable: {error} (settings/state/screenshots/recordings cannot be saved)",
                dir.display()
            ),
        );
    }

    // `logs\` is created by the app's log plugin and by this command; it must be usable too.
    if let Err(error) = fs::create_dir_all(dir.join("logs")) {
        return (
            Status::Warn,
            format!(
                "{} is writable but logs\\ cannot be created: {error}",
                dir.display()
            ),
        );
    }
    match cleanup {
        Ok(()) => (
            Status::Pass,
            format!(
                "{} writable (probe file written, read back and deleted)",
                dir.display()
            ),
        ),
        Err(error) => (
            Status::Warn,
            format!(
                "{} writable but the probe file could not be deleted: {error}",
                dir.display()
            ),
        ),
    }
}

/// `settings.json` exists and parses as the settings object; a corrupt file is recoverable.
fn probe_settings_json() -> (Status, String) {
    probe_document("settings.json", RootKind::Object)
}

/// `state.json` exists and parses as the bare item array; a corrupt file is recoverable.
fn probe_state_json() -> (Status, String) {
    probe_document("state.json", RootKind::Array)
}

/// Shared settings/state probe: read the main document and (when present) a recovery backup.
fn probe_document(file_name: &str, root: RootKind) -> (Status, String) {
    let dir = match data_dir_path() {
        Ok(dir) => dir,
        Err(error) => {
            return (
                Status::Warn,
                format!("{file_name}: cannot inspect, {error}"),
            )
        }
    };
    probe_document_in(&dir, file_name, root)
}

/// [`probe_document`] against an explicit directory (unit-testable with a scratch directory).
fn probe_document_in(dir: &Path, file_name: &str, root: RootKind) -> (Status, String) {
    let main = DocState::from_read(&dir.join(file_name), root);
    // The app itself writes `<name>.corrupt` (settings::backup_corrupt); `.bak` is accepted too so
    // hand-made/older backups are reported as recoverable instead of as data loss.
    let mut backup: Option<(String, DocState)> = None;
    for suffix in [".bak", ".corrupt"] {
        let candidate = dir.join(format!("{file_name}{suffix}"));
        if candidate.exists() {
            backup = Some((
                format!("{file_name}{suffix}"),
                DocState::from_read(&candidate, root),
            ));
            break;
        }
    }
    classify_document(
        file_name,
        main,
        backup
            .as_ref()
            .map(|(label, state)| (label.as_str(), state.clone())),
    )
}

/// Free space on the volume that holds the data directory (< 500 MB is a WARN).
fn probe_disk_free() -> (Status, String) {
    let path = match data_dir_path() {
        Ok(dir) => dir,
        Err(_) => match std::env::var_os("LOCALAPPDATA").filter(|value| !value.is_empty()) {
            Some(base) => PathBuf::from(base),
            None => {
                return (
                    Status::Warn,
                    "LOCALAPPDATA is not set; cannot probe the disk".into(),
                )
            }
        },
    };
    let Some(root) = volume_root(&path) else {
        return (
            Status::Warn,
            format!("cannot derive a drive root from {}", path.display()),
        );
    };
    let wide: Vec<u16> = root.encode_utf16().chain(std::iter::once(0)).collect();
    let mut free_to_caller = 0u64;
    let mut total = 0u64;
    let mut free_total = 0u64;
    // SAFETY: `wide` is a NUL-terminated UTF-16 path that outlives the call; the three output
    // pointers are valid `u64`s for the duration of the call.
    let result = unsafe {
        GetDiskFreeSpaceExW(
            PCWSTR(wide.as_ptr()),
            Some(&mut free_to_caller as *mut u64),
            Some(&mut total as *mut u64),
            Some(&mut free_total as *mut u64),
        )
    };
    if let Err(error) = result {
        return (
            Status::Warn,
            format!("GetDiskFreeSpaceExW({root}) failed: {error}"),
        );
    }
    let free = free_total;
    if free < MIN_FREE_DISK_BYTES {
        (
            Status::Warn,
            format!(
                "{} free of {} on {root} (below {}; new screenshots/recordings may fail)",
                format_bytes(free),
                format_bytes(total),
                format_bytes(MIN_FREE_DISK_BYTES)
            ),
        )
    } else {
        (
            Status::Pass,
            format!(
                "{} free of {} on {root}",
                format_bytes(free),
                format_bytes(total)
            ),
        )
    }
}

/// Effective system DPI and this process's DPI-awareness context.
fn probe_dpi() -> (Status, String) {
    // SAFETY: both calls are read-only queries and take no user pointers.
    let (context, system_dpi) = unsafe { (GetThreadDpiAwarenessContext(), GetDpiForSystem()) };
    let awareness = describe_awareness(context);
    if system_dpi > 0 {
        let scale = f64::from(system_dpi) / 96.0 * 100.0;
        return (
            Status::Pass,
            format!("{system_dpi} dpi ({scale:.0}%), awareness {awareness}"),
        );
    }

    // Fallback for sessions where GetDpiForSystem returns 0: ask a screen DC for LOGPIXELSX.
    // SAFETY: GetDC(None) returns a screen DC that is released below; GetDeviceCaps only reads.
    let hdc = unsafe { GetDC(None) };
    if hdc.is_invalid() {
        return (
            Status::Warn,
            format!("DPI unavailable (awareness {awareness})"),
        );
    }
    let dpi = unsafe { GetDeviceCaps(Some(hdc), LOGPIXELSX) };
    // SAFETY: `hdc` came from GetDC(None) above and is released exactly once here.
    unsafe {
        let _ = ReleaseDC(None, hdc);
    }
    if dpi <= 0 {
        return (
            Status::Warn,
            format!("DPI unavailable (awareness {awareness})"),
        );
    }
    let dpi = dpi as u32;
    let scale = f64::from(dpi) / 96.0 * 100.0;
    (
        Status::Pass,
        format!("{dpi} dpi ({scale:.0}%), awareness {awareness}, source GetDeviceCaps(LOGPIXELSX)"),
    )
}

/// Human-readable DPI-awareness label for the current thread.
fn describe_awareness(context: DPI_AWARENESS_CONTEXT) -> &'static str {
    // SAFETY: AreDpiAwarenessContextsEqual only compares the opaque context handles.
    let is = |expected: DPI_AWARENESS_CONTEXT| unsafe {
        AreDpiAwarenessContextsEqual(context, expected).as_bool()
    };
    if is(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) {
        "per-monitor-v2"
    } else if is(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE) {
        "per-monitor"
    } else if is(DPI_AWARENESS_CONTEXT_SYSTEM_AWARE) {
        "system"
    } else if is(DPI_AWARENESS_CONTEXT_UNAWARE_GDISCALED) {
        "unaware-gdiscaled"
    } else if is(DPI_AWARENESS_CONTEXT_UNAWARE) {
        "unaware"
    } else {
        "unknown"
    }
}

/// Monitor count, primary-screen size and virtual-desktop bounds.
fn probe_display() -> (Status, String) {
    // SAFETY: GetSystemMetrics is a read-only query with no output pointers.
    let (monitors, primary_w, primary_h, virtual_x, virtual_y, virtual_w, virtual_h) = unsafe {
        (
            GetSystemMetrics(SM_CMONITORS),
            GetSystemMetrics(SM_CXSCREEN),
            GetSystemMetrics(SM_CYSCREEN),
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    };
    if monitors <= 0 {
        return (
            Status::Fail,
            "no display monitor reported; screen capture cannot work in this session".into(),
        );
    }
    (
        Status::Pass,
        format!(
            "{monitors} monitor(s); primary {primary_w}x{primary_h}; virtual desktop \
             {virtual_w}x{virtual_h} at ({virtual_x},{virtual_y})"
        ),
    )
}

/// WebView2 Evergreen runtime: highest `msedgewebview2.exe` version under the install roots.
fn probe_webview2() -> (Status, String) {
    let (best, roots_present) = newest_webview2_install(&webview2_roots());
    match best {
        Some((version, path)) => (
            Status::Pass,
            format!("runtime {} at {}", format_version(&version), path.display()),
        ),
        None if roots_present == 0 => (
            Status::Fail,
            format!("no WebView2 install root found; {WEBVIEW2_HINT}"),
        ),
        None => (
            Status::Fail,
            format!("runtime not found under {roots_present} install root(s); {WEBVIEW2_HINT}"),
        ),
    }
}

/// Scans `roots` and returns the highest-version `msedgewebview2.exe` install plus how many of the
/// roots existed at all.
///
/// A directory only counts when it is a real runtime install (`msedgewebview2.exe` inside) and its
/// name parses as a dotted version; the version directories are compared component-wise (missing
/// components count as 0). Takes the roots as an argument so the unit tests can use a scratch tree.
fn newest_webview2_install(roots: &[PathBuf]) -> (Option<(Vec<u64>, PathBuf)>, usize) {
    let mut best: Option<(Vec<u64>, PathBuf)> = None;
    let mut roots_present = 0usize;
    for root in roots {
        let Ok(entries) = fs::read_dir(root) else {
            continue;
        };
        roots_present += 1;
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() || !path.join("msedgewebview2.exe").is_file() {
                continue;
            }
            let Some(version) = entry.file_name().to_str().and_then(parse_version) else {
                continue;
            };
            let newer = best
                .as_ref()
                .map_or(true, |(best_version, _)| version_gt(&version, best_version));
            if newer {
                best = Some((version, path));
            }
        }
    }
    (best, roots_present)
}

/// Where the WebView2 Evergreen runtime can be installed (machine-wide + per-user).
fn webview2_roots() -> Vec<PathBuf> {
    let mut program_files = Vec::new();
    for variable in ["ProgramFiles(x86)", "ProgramFiles"] {
        if let Some(value) = std::env::var_os(variable).filter(|value| !value.is_empty()) {
            program_files.push(PathBuf::from(value));
        }
    }
    if program_files.is_empty() {
        program_files.push(PathBuf::from(r"C:\Program Files (x86)"));
        program_files.push(PathBuf::from(r"C:\Program Files"));
    }
    let mut roots: Vec<PathBuf> = program_files
        .into_iter()
        .map(|base| base.join(WEBVIEW2_APP_SUBDIR))
        .collect();
    if let Some(local) = std::env::var_os("LOCALAPPDATA").filter(|value| !value.is_empty()) {
        roots.push(PathBuf::from(local).join(WEBVIEW2_APP_SUBDIR));
    }
    roots
}

/// Process memory baseline (working set) plus the handle count.
fn probe_memory() -> (Status, String) {
    let handles = current_process_handles()
        .map(|count| format!("{count} handle(s)"))
        .unwrap_or_else(|| "handles unavailable".to_string());
    match process_memory_counters() {
        Some(counters) => (
            Status::Pass,
            format!(
                "working set {} (peak {}), {handles}",
                format_bytes(counters.working_set_size as u64),
                format_bytes(counters.peak_working_set_size as u64),
            ),
        ),
        None => (
            Status::Warn,
            format!("working set unavailable (K32GetProcessMemoryInfo failed), {handles}"),
        ),
    }
}

/// Handle count of the current process (`GetProcessHandleCount`, enabled feature).
fn current_process_handles() -> Option<u32> {
    let mut count = 0u32;
    // SAFETY: the pseudo handle from GetCurrentProcess needs no cleanup; `count` is a valid out
    // pointer for the duration of the call.
    let result = unsafe { GetProcessHandleCount(GetCurrentProcess(), &mut count) };
    result.ok().map(|()| count)
}

/// Working-set counters of the current process, `None` when the Win32 call fails.
fn process_memory_counters() -> Option<ProcessMemoryCounters> {
    let mut counters = ProcessMemoryCounters {
        cb: std::mem::size_of::<ProcessMemoryCounters>() as u32,
        ..ProcessMemoryCounters::default()
    };
    let cb = counters.cb;
    // SAFETY: `counters` is a zeroed, correctly sized PROCESS_MEMORY_COUNTERS whose `cb` is the
    // struct size; K32GetProcessMemoryInfo only writes into it and the pseudo handle needs no
    // cleanup.
    let ok = unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, cb) };
    ok.as_bool().then_some(counters)
}

/// `PROCESS_MEMORY_COUNTERS` from psapi.h (x64 layout; `usize` == `SIZE_T`).
#[repr(C)]
#[derive(Default)]
struct ProcessMemoryCounters {
    cb: u32,
    page_fault_count: u32,
    peak_working_set_size: usize,
    working_set_size: usize,
    quota_peak_paged_pool_usage: usize,
    quota_paged_pool_usage: usize,
    quota_peak_non_paged_pool_usage: usize,
    quota_non_paged_pool_usage: usize,
    pagefile_usage: usize,
    peak_pagefile_usage: usize,
}

// `GetProcessMemoryInfo` lives in `Win32::System::ProcessStatus`, a crate feature this binary does
// not enable. kernel32 exports the same function as `K32GetProcessMemoryInfo`; declaring it here
// keeps Cargo.toml untouched (kernel32 is linked into every Windows binary anyway).
#[link(name = "kernel32")]
unsafe extern "system" {
    fn K32GetProcessMemoryInfo(
        process: HANDLE,
        counters: *mut ProcessMemoryCounters,
        cb: u32,
    ) -> BOOL;
}

/// `HKCU\...\Run` login item (read-only; missing means "Start at login" is off → WARN).
fn probe_autostart() -> (Status, String) {
    match read_run_value(AUTOSTART_VALUE) {
        Ok(Some(command)) => (
            Status::Pass,
            format!("HKCU Run value {AUTOSTART_VALUE} = {command}"),
        ),
        Ok(None) => (
            Status::Warn,
            "Hangline is not registered in HKCU\\...\\Run (\"Start at login\" is off)".to_string(),
        ),
        Err(error) => (Status::Warn, format!("cannot read HKCU\\...\\Run: {error}")),
    }
}

/// Build version, exe path and build profile.
fn probe_version() -> (Status, String) {
    let build = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    let exe = std::env::current_exe()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|error| format!("<unavailable: {error}>"));
    (
        Status::Pass,
        format!(
            "v{} ({build}, {}-{}) exe {exe}",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::ARCH,
            std::env::consts::OS
        ),
    )
}

// ---------------------------------------------------------------------------------------------
// Win32 helpers
// ---------------------------------------------------------------------------------------------

/// `%LOCALAPPDATA%\Hangline` (the app's own data dir), without creating it.
fn data_dir_path() -> Result<PathBuf, String> {
    let base = std::env::var_os("LOCALAPPDATA")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            "LOCALAPPDATA is not set; cannot locate %LOCALAPPDATA%\\Hangline".to_string()
        })?;
    Ok(PathBuf::from(base).join("Hangline"))
}

/// Drive root (`C:\`) of an absolute path, used as the `GetDiskFreeSpaceExW` argument.
fn volume_root(path: &Path) -> Option<String> {
    for component in path.components() {
        if let Component::Prefix(prefix) = component {
            let mut root = prefix.as_os_str().to_string_lossy().into_owned();
            if !root.ends_with('\\') {
                root.push('\\');
            }
            return Some(root);
        }
    }
    None
}

/// NUL-terminated UTF-16 buffer for the `W` registry/file Win32 calls.
fn to_wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Reads the `Run` command of `value_name` from `HKCU`; `None` = the value does not exist.
fn read_run_value(value_name: &str) -> Result<Option<String>, String> {
    let key_path = to_wide(RUN_KEY);
    let mut key = HKEY::default();
    // SAFETY: `key_path` is NUL-terminated and outlives the call; `key` is a valid out pointer.
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(key_path.as_ptr()),
            None,
            KEY_QUERY_VALUE,
            &mut key,
        )
    };
    if status == ERROR_FILE_NOT_FOUND || status == ERROR_PATH_NOT_FOUND {
        return Ok(None);
    }
    if status != ERROR_SUCCESS {
        return Err(format!(
            "RegOpenKeyExW failed with Win32 error {}",
            status.0
        ));
    }
    let result = query_registry_string(key, value_name);
    // SAFETY: `key` was opened above and is closed exactly once here.
    unsafe {
        let _ = RegCloseKey(key);
    }
    result
}

/// Reads one string value from an open registry key.
fn query_registry_string(key: HKEY, value_name: &str) -> Result<Option<String>, String> {
    let name = to_wide(value_name);
    let mut kind = REG_VALUE_TYPE::default();
    let mut size = 0u32;
    // SAFETY: `name` is NUL-terminated and outlives both calls; the out pointers are valid.
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
        return Ok(None);
    }
    if status != ERROR_SUCCESS {
        return Err(format!(
            "RegQueryValueExW(size) failed with Win32 error {}",
            status.0
        ));
    }
    let mut data = vec![0u8; size as usize];
    let mut written = size;
    // SAFETY: `data` is `size` bytes long, which the first call reported as sufficient.
    let status = unsafe {
        RegQueryValueExW(
            key,
            PCWSTR(name.as_ptr()),
            None,
            None,
            Some(data.as_mut_ptr()),
            Some(&mut written),
        )
    };
    if status != ERROR_SUCCESS {
        return Err(format!(
            "RegQueryValueExW(data) failed with Win32 error {}",
            status.0
        ));
    }
    data.truncate(written as usize);
    decode_registry_string(kind, &data).map(Some)
}

/// Decodes a `REG_SZ`/`REG_EXPAND_SZ` value (UTF-16LE, NUL-terminated at the first `0` unit).
fn decode_registry_string(kind: REG_VALUE_TYPE, bytes: &[u8]) -> Result<String, String> {
    if kind != REG_SZ && kind != REG_EXPAND_SZ {
        return Err(format!(
            "registry value has type {} (expected REG_SZ)",
            kind.0
        ));
    }
    if bytes.len() % 2 != 0 {
        return Err("registry string has an odd number of UTF-16 bytes".to_string());
    }
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    let end = units
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(units.len());
    Ok(String::from_utf16_lossy(&units[..end]))
}

// ---------------------------------------------------------------------------------------------
// Pure helpers (unit-tested without touching the real registry / data directory)
// ---------------------------------------------------------------------------------------------

/// Formatting helper shared by the report (bytes → `1.5 MB`).
fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;
    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

/// Parses a dotted numeric version (`130.0.2849.68`) into comparable components.
fn parse_version(value: &str) -> Option<Vec<u64>> {
    let components = value
        .split('.')
        .map(|component| component.parse::<u64>().ok())
        .collect::<Option<Vec<u64>>>()?;
    (components.len() >= 2).then_some(components)
}

/// `true` when `left` is a strictly newer version than `right` (missing components count as 0).
fn version_gt(left: &[u64], right: &[u64]) -> bool {
    let len = left.len().max(right.len());
    for index in 0..len {
        let lhs = left.get(index).copied().unwrap_or(0);
        let rhs = right.get(index).copied().unwrap_or(0);
        if lhs != rhs {
            return lhs > rhs;
        }
    }
    false
}

/// Renders a parsed version back to dotted form.
fn format_version(version: &[u64]) -> String {
    version
        .iter()
        .map(u64::to_string)
        .collect::<Vec<String>>()
        .join(".")
}

/// Expected JSON root of a persisted document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RootKind {
    /// `settings.json` is a JSON object.
    Object,
    /// `state.json` is a bare JSON array of items.
    Array,
}

impl RootKind {
    /// Short "parsed shape" summary, or `None` when the document has the wrong root shape.
    fn describe(self, value: &Value) -> Option<String> {
        match (self, value) {
            (RootKind::Object, Value::Object(map)) => {
                Some(format!("object with {} key(s)", map.len()))
            }
            (RootKind::Array, Value::Array(items)) => {
                Some(format!("array with {} item(s)", items.len()))
            }
            _ => None,
        }
    }
}

/// What a persisted document (or its backup) looks like on disk.
#[derive(Clone, Debug, PartialEq, Eq)]
enum DocState {
    /// File does not exist (normal on first run).
    Missing,
    /// File exists, is valid JSON and has the expected root shape.
    Ok {
        /// File size in bytes.
        bytes: u64,
        /// Parsed-shape summary from [`RootKind::describe`].
        summary: String,
    },
    /// File exists but is not valid JSON.
    BadJson,
    /// File is valid JSON but not the expected root shape (the app would reject it too).
    BadShape,
    /// File exists but could not be read (permissions, I/O error).
    Unreadable(String),
}

impl DocState {
    /// Reads and classifies `path`.
    fn from_read(path: &Path, root: RootKind) -> Self {
        match fs::read(path) {
            Ok(bytes) => DocState::from_bytes(&bytes, root),
            Err(error) if error.kind() == io::ErrorKind::NotFound => DocState::Missing,
            Err(error) => DocState::Unreadable(error.to_string()),
        }
    }

    /// Classifies already-read bytes (pure, used by the unit tests).
    fn from_bytes(bytes: &[u8], root: RootKind) -> Self {
        match serde_json::from_slice::<Value>(bytes) {
            Ok(value) => match root.describe(&value) {
                Some(summary) => DocState::Ok {
                    bytes: bytes.len() as u64,
                    summary,
                },
                None => DocState::BadShape,
            },
            Err(_) => DocState::BadJson,
        }
    }
}

/// Turns the observed main/backup states into a status + detail.
///
/// Recovery rule: a corrupt or missing main file is a WARN when a parseable backup exists (the app
/// or the user can restore it) and a FAIL when nothing usable is left (the app will silently reset).
fn classify_document(
    file_name: &str,
    main: DocState,
    backup: Option<(&str, DocState)>,
) -> (Status, String) {
    let usable_backup: Option<(&str, &DocState)> = match &backup {
        Some((label, state @ DocState::Ok { .. })) => Some((*label, state)),
        _ => None,
    };
    match &main {
        DocState::Ok { bytes, summary } => (
            Status::Pass,
            format!("{file_name} ok ({}, {summary})", format_bytes(*bytes)),
        ),
        DocState::Missing => match usable_backup {
            Some((label, DocState::Ok { summary, .. })) => (
                Status::Warn,
                format!("{file_name} is missing; {label} parses ({summary}), restore is possible"),
            ),
            _ => (
                Status::Warn,
                format!("{file_name} is missing (first run; the app seeds defaults)"),
            ),
        },
        DocState::Unreadable(error) => (
            Status::Warn,
            format!(
                "{file_name} is unreadable ({error}); the app falls back to in-memory defaults"
            ),
        ),
        DocState::BadJson | DocState::BadShape => {
            let problem = match &main {
                DocState::BadJson => "not valid JSON",
                DocState::BadShape => "JSON with the wrong root shape",
                _ => unreachable!("the outer match only enters this arm for bad documents"),
            };
            match usable_backup {
                Some((label, DocState::Ok { summary, .. })) => (
                    Status::Warn,
                    format!(
                        "{file_name} is {problem}; {label} parses ({summary}), restore is possible"
                    ),
                ),
                _ => {
                    let backup_note = match &backup {
                        Some((label, _)) => format!(" and {label} is unusable"),
                        None => " and no backup exists".to_string(),
                    };
                    (
                        Status::Fail,
                        format!(
                            "{file_name} is {problem}{backup_note}; the app will replace it with \
                             defaults"
                        ),
                    )
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a settings-like or state-like document state for the classifier tests.
    fn json_state(bytes: &[u8], root: RootKind) -> DocState {
        DocState::from_bytes(bytes, root)
    }

    fn check(name: &'static str, status: Status, detail: &str) -> Check {
        Check::new(name, status, detail)
    }

    /// Scratch directory under `target/` (git-ignored); never touches `%LOCALAPPDATA%`.
    fn test_dir(tag: &str) -> PathBuf {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("selftest-tests")
            .join(format!("{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create the selftest test dir");
        dir
    }

    // ---- CLI parsing (the normal run path must stay untouched) -------------------------------

    #[test]
    fn selftest_parse_mode_empty_is_normal() {
        assert_eq!(parse_mode(Vec::<String>::new()), Mode::Normal);
        assert_eq!(parse_mode(std::iter::empty::<&str>()), Mode::Normal);
    }

    #[test]
    fn selftest_parse_mode_plain_args_are_normal() {
        assert_eq!(parse_mode(["--profile", r"C:\Shots"]), Mode::Normal);
        assert_eq!(parse_mode(["hangline.exe"]), Mode::Normal);
        assert_eq!(parse_mode(["selftest"]), Mode::Normal);
        assert_eq!(parse_mode(["--selftest=1"]), Mode::Normal);
    }

    #[test]
    fn selftest_parse_mode_selftest_flag() {
        assert_eq!(parse_mode(["--selftest"]), Mode::SelfTest { json: false });
    }

    #[test]
    fn selftest_parse_mode_json_alone_does_not_start_selftest() {
        assert_eq!(parse_mode(["--json"]), Mode::Normal);
    }

    #[test]
    fn selftest_parse_mode_selftest_with_json() {
        assert_eq!(
            parse_mode(["--selftest", "--json"]),
            Mode::SelfTest { json: true }
        );
        assert_eq!(
            parse_mode(["--json", "--selftest"]),
            Mode::SelfTest { json: true }
        );
    }

    // ---- Summary / exit code ----------------------------------------------------------------

    #[test]
    fn selftest_summary_counts_and_exit_code() {
        let checks = vec![
            check("a", Status::Pass, "ok"),
            check("b", Status::Pass, "ok"),
            check("c", Status::Warn, "meh"),
            check("d", Status::Fail, "broken"),
        ];
        let summary = Summary::from_checks(&checks);
        assert_eq!(
            summary,
            Summary {
                pass: 2,
                warn: 1,
                fail: 1,
            }
        );
        // WARN never contributes to the exit code; the FAIL count is the exit code.
        assert_eq!(summary.exit_code(), 1);
    }

    #[test]
    fn selftest_summary_all_pass_exits_zero() {
        let checks = vec![
            check("a", Status::Pass, "ok"),
            check("b", Status::Warn, "meh"),
        ];
        assert_eq!(Summary::from_checks(&checks).exit_code(), 0);
    }

    #[test]
    fn selftest_summary_exit_code_is_clamped_to_255() {
        let checks: Vec<Check> = (0..300)
            .map(|_| check("f", Status::Fail, "broken"))
            .collect();
        assert_eq!(Summary::from_checks(&checks).exit_code(), 255);
    }

    // ---- Probe table / panic isolation ------------------------------------------------------

    #[test]
    fn selftest_probe_table_is_well_formed() {
        assert!(
            (8..=12).contains(&PROBES.len()),
            "the task asks for 8-12 probes, found {}",
            PROBES.len()
        );
        let mut names: Vec<&str> = PROBES.iter().map(|&(name, _)| name).collect();
        names.sort_unstable();
        let mut unique = names.clone();
        unique.dedup();
        assert_eq!(names, unique, "probe names must be unique");
        assert_eq!(
            names,
            vec![
                "autostart",
                "data-dir",
                "disk-free",
                "display",
                "dpi",
                "memory",
                "settings-json",
                "state-json",
                "version",
                "webview2",
            ]
        );
    }

    #[test]
    fn selftest_panicking_probe_becomes_a_fail() {
        fn panic_probe() -> (Status, String) {
            panic!("simulated probe failure");
        }
        let converted = probe_into_check("memory", panic_probe);
        assert_eq!(converted.name, "memory");
        assert_eq!(converted.status, Status::Fail);
        assert!(converted.detail.contains("panicked"));
    }

    // ---- Rendering --------------------------------------------------------------------------

    fn sample_checks() -> Vec<Check> {
        vec![
            check("data-dir", Status::Pass, "C:\\data writable"),
            check("webview2", Status::Fail, "runtime not found"),
        ]
    }

    #[test]
    fn selftest_render_text_has_one_line_per_check_and_a_summary() {
        let checks = sample_checks();
        let summary = Summary::from_checks(&checks);
        let report = render_text(&checks, summary, "2026-11-09 13:46:09.000 (local)");
        let lines: Vec<&str> = report.lines().collect();
        assert!(lines[0].starts_with("Hangline selftest v"));
        assert!(report.contains("time: 2026-11-09 13:46:09.000 (local)"));
        assert!(lines.iter().any(|line| line.starts_with("[PASS] data-dir")));
        assert!(lines.iter().any(|line| line.starts_with("[FAIL] webview2")));
        let last = lines.last().expect("summary line");
        assert_eq!(last, &"PASS 1  WARN 0  FAIL 1  exit code 1");
    }

    #[test]
    fn selftest_render_json_is_one_object_per_check_plus_summary() {
        let checks = sample_checks();
        let summary = Summary::from_checks(&checks);
        let report = render_json(&checks, summary);
        let lines: Vec<&str> = report.lines().collect();
        assert_eq!(lines.len(), checks.len() + 1);

        let first: Value = serde_json::from_str(lines[0]).expect("check line is JSON");
        assert_eq!(first["name"], "data-dir");
        assert_eq!(first["status"], "PASS");
        assert_eq!(first["detail"], "C:\\data writable");
        assert_eq!(first.as_object().expect("object").len(), 3);

        let second: Value = serde_json::from_str(lines[1]).expect("check line is JSON");
        assert_eq!(second["status"], "FAIL");

        let summary_value: Value = serde_json::from_str(lines[2]).expect("summary line is JSON");
        assert_eq!(summary_value["summary"]["pass"], 1);
        assert_eq!(summary_value["summary"]["warn"], 0);
        assert_eq!(summary_value["summary"]["fail"], 1);
        assert_eq!(summary_value["summary"]["exitCode"], 1);
    }

    #[test]
    fn selftest_render_json_escapes_details() {
        // Quotes, backslashes and newlines must round-trip through the JSON line unchanged.
        let detail = "path \"odd\" C:\\temp\nline2";
        let checks = vec![check("disk-free", Status::Warn, detail)];
        let summary = Summary::from_checks(&checks);
        let report = render_json(&checks, summary);
        let line: Value =
            serde_json::from_str(report.lines().next().expect("check line")).expect("valid JSON");
        assert_eq!(line["detail"], detail);
    }

    // ---- Document classification ------------------------------------------------------------

    #[test]
    fn selftest_doc_state_classifies_json_shapes() {
        let object = json_state(br#"{"keepForDays":7}"#, RootKind::Object);
        assert!(
            matches!(&object, DocState::Ok { summary, .. } if summary == "object with 1 key(s)"),
            "unexpected state: {object:?}"
        );
        let array = json_state(br#"[{"id":"a"}]"#, RootKind::Array);
        assert!(
            matches!(&array, DocState::Ok { summary, .. } if summary == "array with 1 item(s)"),
            "unexpected state: {array:?}"
        );
        // root shape mismatch: valid JSON but the wrong document kind
        assert_eq!(
            json_state(br#"{"keepForDays":7}"#, RootKind::Array),
            DocState::BadShape
        );
        assert_eq!(
            json_state(br#"[{"id":"a"}]"#, RootKind::Object),
            DocState::BadShape
        );
        assert_eq!(
            json_state(b"not json at all", RootKind::Object),
            DocState::BadJson
        );
    }

    #[test]
    fn selftest_classify_document_healthy() {
        let (status, detail) = classify_document(
            "settings.json",
            json_state(br#"{"keepForDays":7}"#, RootKind::Object),
            None,
        );
        assert_eq!(status, Status::Pass);
        assert!(detail.contains("settings.json ok"));
        assert!(detail.contains("object with 1 key(s)"));
    }

    #[test]
    fn selftest_classify_document_missing_first_run_is_warn() {
        let (status, detail) = classify_document("state.json", DocState::Missing, None);
        assert_eq!(status, Status::Warn);
        assert!(detail.contains("missing"));
    }

    #[test]
    fn selftest_classify_document_missing_with_backup_is_recoverable_warn() {
        let (status, detail) = classify_document(
            "settings.json",
            DocState::Missing,
            Some((
                "settings.json.bak",
                json_state(br#"{"keepForDays":7}"#, RootKind::Object),
            )),
        );
        assert_eq!(status, Status::Warn);
        assert!(detail.contains("settings.json.bak"));
        assert!(detail.contains("restore is possible"));
    }

    #[test]
    fn selftest_classify_document_corrupt_with_parseable_backup_is_warn() {
        let (status, detail) = classify_document(
            "settings.json",
            DocState::BadJson,
            Some((
                "settings.json.corrupt",
                json_state(br#"{"keepForDays":7}"#, RootKind::Object),
            )),
        );
        assert_eq!(status, Status::Warn);
        assert!(detail.contains("settings.json.corrupt"));
        assert!(detail.contains("restore is possible"));
    }

    #[test]
    fn selftest_classify_document_corrupt_without_backup_fails() {
        let (status, detail) = classify_document("settings.json", DocState::BadJson, None);
        assert_eq!(status, Status::Fail);
        assert!(detail.contains("no backup exists"));
    }

    #[test]
    fn selftest_classify_document_corrupt_with_unusable_backup_fails() {
        let (status, detail) = classify_document(
            "state.json",
            DocState::BadShape,
            Some(("state.json.bak", DocState::BadJson)),
        );
        assert_eq!(status, Status::Fail);
        assert!(detail.contains("unusable"));
    }

    #[test]
    fn selftest_classify_document_unreadable_is_warn() {
        let (status, detail) = classify_document(
            "settings.json",
            DocState::Unreadable("access denied".to_string()),
            None,
        );
        assert_eq!(status, Status::Warn);
        assert!(detail.contains("access denied"));
    }

    // ---- Small pure helpers -----------------------------------------------------------------

    #[test]
    fn selftest_format_bytes_thresholds() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(999), "999 B");
        assert_eq!(format_bytes(1024), "1.0 KB");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(500 * 1024 * 1024), "500.0 MB");
        assert_eq!(format_bytes(2 * 1024 * 1024 * 1024), "2.0 GB");
    }

    #[test]
    fn selftest_format_version_and_compare() {
        assert_eq!(parse_version("130.0.2849.68"), Some(vec![130, 0, 2849, 68]));
        assert_eq!(parse_version("130.0"), Some(vec![130, 0]));
        assert_eq!(parse_version("130"), None);
        assert_eq!(parse_version("not-a-version"), None);
        assert_eq!(parse_version("130.0.2849.x"), None);

        assert!(version_gt(&[130, 0, 2849, 68], &[129, 0, 1, 1]));
        assert!(version_gt(&[130, 0, 2849, 69], &[130, 0, 2849, 68]));
        assert!(!version_gt(&[130, 0, 2849, 68], &[130, 0, 2849, 68]));
        assert!(!version_gt(&[1, 2], &[1, 2, 0]));

        assert_eq!(format_version(&[130, 0, 2849, 68]), "130.0.2849.68");
    }

    #[test]
    fn selftest_volume_root_extracts_drive_letter() {
        assert_eq!(
            volume_root(Path::new(r"C:\Users\Example\AppData\Local\Hangline")),
            Some(r"C:\".to_string())
        );
        assert_eq!(volume_root(Path::new("relative\\path")), None);
    }

    #[test]
    fn selftest_format_system_time() {
        let time = SYSTEMTIME {
            wYear: 2026,
            wMonth: 11,
            wDayOfWeek: 1,
            wDay: 9,
            wHour: 13,
            wMinute: 46,
            wSecond: 9,
            wMilliseconds: 5,
        };
        assert_eq!(format_system_time(&time), "2026-11-09 13:46:09.005 (local)");
    }

    #[test]
    fn selftest_decode_registry_string_utf16() {
        let encode = |value: &str| -> Vec<u8> {
            value
                .encode_utf16()
                .flat_map(|unit| unit.to_le_bytes())
                .collect()
        };
        let mut bytes = encode("Hangline");
        bytes.extend_from_slice(&[0, 0]);
        assert_eq!(decode_registry_string(REG_SZ, &bytes).unwrap(), "Hangline");
        assert_eq!(
            decode_registry_string(REG_EXPAND_SZ, &encode("晾衣绳")).unwrap(),
            "晾衣绳"
        );
        assert!(decode_registry_string(REG_SZ, &[0x48]).is_err());
        let binary = windows::Win32::System::Registry::REG_BINARY;
        assert!(decode_registry_string(binary, &encode("Hangline")).is_err());
    }

    // ---- Filesystem probes against a scratch tree (no %LOCALAPPDATA% / registry access) -------

    #[test]
    fn selftest_check_data_dir_writable_accepts_a_writable_dir() {
        let base = test_dir("data-dir-ok");
        let dir = base.join("Hangline");
        let (status, detail) = check_data_dir_writable(&dir);
        assert_eq!(status, Status::Pass, "detail: {detail}");
        assert!(detail.contains("writable"));
        assert!(dir.join("logs").is_dir(), "logs\\ must be created");
        // The probe file must never be left behind.
        let leftovers: Vec<String> = fs::read_dir(&dir)
            .expect("list the probe dir")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with(".selftest-probe"))
            .collect();
        assert!(leftovers.is_empty(), "leftover probe files: {leftovers:?}");
    }

    #[test]
    fn selftest_check_data_dir_writable_reports_a_blocked_dir() {
        let base = test_dir("data-dir-blocked");
        let blocker = base.join("blocker");
        fs::write(&blocker, b"a file, not a directory").expect("write the blocker file");
        let (status, detail) = check_data_dir_writable(&blocker.join("Hangline"));
        assert_eq!(status, Status::Fail, "detail: {detail}");
        assert!(detail.contains("cannot create"));
    }

    #[test]
    fn selftest_append_log_to_appends_timestamped_reports() {
        let base = test_dir("log");
        let path = base.join("logs").join("selftest.log");
        append_log_to(
            &path,
            "first report\n",
            "2026-11-09 13:46:09.000 (local)",
            0,
        )
        .expect("first append");
        append_log_to(&path, "second report", "2026-11-09 13:46:10.000 (local)", 2)
            .expect("second append");
        let content = fs::read_to_string(&path).expect("read the log back");
        assert_eq!(content.matches("===== Hangline selftest ").count(), 2);
        assert!(content.contains("2026-11-09 13:46:09.000 (local)"));
        assert!(content.contains("exit=0 ====="));
        assert!(content.contains("exit=2 ====="));
        assert!(content.contains("first report"));
        assert!(content.contains("second report\n"));
    }

    #[test]
    fn selftest_probe_document_in_handles_healthy_recoverable_and_corrupt_files() {
        let dir = test_dir("documents");
        let settings = dir.join("settings.json");
        fs::write(&settings, br#"{"keepForDays":7}"#).expect("write healthy settings");

        let (status, detail) = probe_document_in(&dir, "settings.json", RootKind::Object);
        assert_eq!(status, Status::Pass, "detail: {detail}");
        assert!(detail.contains("object with 1 key(s)"));

        // Corrupt main file + parseable backup: recoverable, not data loss.
        fs::write(&settings, b"not json").expect("corrupt the main file");
        fs::write(dir.join("settings.json.corrupt"), br#"{"keepForDays":7}"#)
            .expect("write the backup");
        let (status, detail) = probe_document_in(&dir, "settings.json", RootKind::Object);
        assert_eq!(status, Status::Warn, "detail: {detail}");
        assert!(detail.contains("restore is possible"));

        // Corrupt main file, nothing usable left: FAIL.
        fs::remove_file(dir.join("settings.json.corrupt")).expect("remove the backup");
        let (status, detail) = probe_document_in(&dir, "settings.json", RootKind::Object);
        assert_eq!(status, Status::Fail, "detail: {detail}");
        assert!(detail.contains("no backup exists"));

        // Missing file is the first-run case: WARN only.
        fs::remove_file(&settings).expect("remove the main file");
        let (status, _) = probe_document_in(&dir, "settings.json", RootKind::Object);
        assert_eq!(status, Status::Warn);
    }

    #[test]
    fn selftest_newest_webview2_install_picks_the_highest_version() {
        let base = test_dir("webview2");
        let root_a = base.join("root-a");
        let root_b = base.join("root-b");
        for (root, version, with_exe) in [
            (&root_a, "120.0.2210.91", true),
            (&root_a, "121.0.1.1", false), // no msedgewebview2.exe -> ignored
            (&root_b, "124.0.0.1", true),
            (&root_b, "not-a-version", true), // unparseable -> ignored
        ] {
            let dir = root.join(version);
            fs::create_dir_all(&dir).expect("create a fake version dir");
            if with_exe {
                fs::write(dir.join("msedgewebview2.exe"), b"stub").expect("write the exe stub");
            }
        }

        let missing = base.join("missing-root");
        let expected_best = root_b.join("124.0.0.1");
        let (best, roots_present) = newest_webview2_install(&[root_a, root_b, missing]);
        assert_eq!(roots_present, 2);
        let (version, path) = best.expect("an install must be found");
        assert_eq!(version, vec![124, 0, 0, 1]);
        assert_eq!(path, expected_best);

        // No roots at all: no install and roots_present == 0 (the probe turns that into a FAIL).
        let (none, roots_present) = newest_webview2_install(&[base.join("absent-a")]);
        assert!(none.is_none());
        assert_eq!(roots_present, 0);
    }

    // ---- Live run (opt-in; never part of the default test run) ------------------------------

    /// End-to-end run of [`run`] against the real machine, for field evidence:
    /// `cargo test --manifest-path src-tauri/Cargo.toml selftest -- --ignored --nocapture`.
    ///
    /// Ignored by default because it touches the real `%LOCALAPPDATA%\Hangline` (probe file +
    /// log append) and reads `HKCU`; the unit tests above cover every pure part.
    #[test]
    #[ignore = "live probes: writes %LOCALAPPDATA%\\Hangline probe file/log and reads HKCU"]
    fn selftest_live_probe_run() {
        let exit_code = run(false);
        assert!(
            (0..=PROBES.len() as i32).contains(&exit_code),
            "exit code {exit_code} must be the FAIL count of {} probes",
            PROBES.len()
        );
    }

    /// Same live run in `--json` mode, to eyeball/validate the one-line-per-check output.
    #[test]
    #[ignore = "live probes: writes %LOCALAPPDATA%\\Hangline probe file/log and reads HKCU"]
    fn selftest_live_json_probe_run() {
        let exit_code = run(true);
        assert!(
            (0..=PROBES.len() as i32).contains(&exit_code),
            "exit code {exit_code} must be the FAIL count of {} probes",
            PROBES.len()
        );
    }
}
