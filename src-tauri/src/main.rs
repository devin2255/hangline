// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// Field diagnostics (`--selftest`) live in the binary crate on purpose: the lib crate owns the app
// runtime and does not expose its private modules, so the probes are implemented independently in
// `selftest.rs` (std + the already-declared `windows`/`serde_json` dependencies; no new crates and
// no changes to the lib). `#[path]` keeps the module a plain file next to `main.rs`.
#[path = "selftest.rs"]
mod selftest;

use std::ffi::OsString;
use std::path::PathBuf;

fn main() {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    // Wave 2.5 OCR evaluation probe: no Tauri build, no window, no WebView2. Every image in the
    // directory is recognized and printed as one JSON line, which is what the evaluation script
    // consumes; the exit code is the number of failed files (capped at 1).
    if let Some(probe) = parse_ocr_probe_args(&args) {
        std::process::exit(match probe {
            Ok(dir) => hangline_lib::ocr::run_probe(&dir),
            Err(error) => {
                eprintln!("hangline: {error}");
                1
            }
        });
    }
    // The remaining command line is only inspected for the diagnostics flags. Without `--selftest`
    // this is the previous `hangline_lib::run()` start line-for-line: no Tauri build, I/O or logging
    // happens before it, so normal launches are unaffected.
    match selftest::parse_mode(args.into_iter()) {
        selftest::Mode::SelfTest { json } => {
            // No window, no tray, no WebView2: run the probes, then exit with the FAIL count
            // (0 = healthy). The report is printed to stdout and appended to
            // `%LOCALAPPDATA%\Hangline\logs\selftest.log`.
            std::process::exit(selftest::run(json));
        }
        selftest::Mode::Normal => hangline_lib::run(),
    }
}

/// Parses the Wave 2.5 `--ocr-probe <dir>` flag.
///
/// Returns `None` when the flag is absent (normal launch or `--selftest`), `Some(Ok(dir))` for a
/// valid invocation and `Some(Err(message))` when the directory argument is missing. The parse is
/// deliberately separate from the Tauri/main path so it is unit-tested in the binary crate.
fn parse_ocr_probe_args(args: &[OsString]) -> Option<Result<PathBuf, &'static str>> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "--ocr-probe" {
            return Some(match iter.next() {
                Some(dir) if !dir.is_empty() => Ok(PathBuf::from(dir)),
                _ => Err("--ocr-probe requires a directory argument"),
            });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    /// W2.5: `--ocr-probe <dir>` is parsed before the selftest/normal launch paths; an absent flag
    /// keeps the existing behavior and a missing directory argument is a diagnostic error.
    #[test]
    fn ocr_probe_arguments_are_parsed_without_touching_the_launch_paths() {
        assert_eq!(
            parse_ocr_probe_args(&args(&["--ocr-probe", r"C:\shots"])),
            Some(Ok(PathBuf::from(r"C:\shots")))
        );
        assert_eq!(
            parse_ocr_probe_args(&args(&["--ocr-probe"])),
            Some(Err("--ocr-probe requires a directory argument"))
        );
        assert_eq!(
            parse_ocr_probe_args(&args(&["--ocr-probe", ""])),
            Some(Err("--ocr-probe requires a directory argument")),
            "an empty path is not a usable directory"
        );
        assert_eq!(parse_ocr_probe_args(&args(&[])), None);
        assert_eq!(parse_ocr_probe_args(&args(&["--selftest"])), None);
        assert_eq!(
            parse_ocr_probe_args(&args(&["--selftest", "--json"])),
            None
        );
        assert_eq!(
            parse_ocr_probe_args(&args(&["--ocr-probe", r"D:\a b\shots"])),
            Some(Ok(PathBuf::from(r"D:\a b\shots"))),
            "paths with spaces stay one argument"
        );
    }
}
