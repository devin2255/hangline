// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// Field diagnostics (`--selftest`) live in the binary crate on purpose: the lib crate owns the app
// runtime and does not expose its private modules, so the probes are implemented independently in
// `selftest.rs` (std + the already-declared `windows`/`serde_json` dependencies; no new crates and
// no changes to the lib). `#[path]` keeps the module a plain file next to `main.rs`.
#[path = "selftest.rs"]
mod selftest;

fn main() {
    // The command line is only inspected for the two diagnostics flags. Without `--selftest` this
    // is the previous `hangline_lib::run()` start line-for-line: no Tauri build, I/O or logging
    // happens before it, so normal launches are unaffected.
    match selftest::parse_mode(std::env::args_os().skip(1)) {
        selftest::Mode::SelfTest { json } => {
            // No window, no tray, no WebView2: run the probes, then exit with the FAIL count
            // (0 = healthy). The report is printed to stdout and appended to
            // `%LOCALAPPDATA%\Hangline\logs\selftest.log`.
            std::process::exit(selftest::run(json));
        }
        selftest::Mode::Normal => hangline_lib::run(),
    }
}
