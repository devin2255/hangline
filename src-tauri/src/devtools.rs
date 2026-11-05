//! Debug-only WebView2 DevTools hook for runtime evidence (controller plan, 2026-11-02).
//!
//! `wry` 0.57.1 always passes its own explicit default `additional_browser_args`, so the documented
//! `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS` environment variable is ignored. Debug builds therefore
//! read `HANG_CDP_PORT` here and every webview builder hands the resulting switch to WebView2;
//! release builds compile the whole hook out ([`cdp_browser_args`] returns `None`), so a shipping
//! binary can never expose a remote debugging port even if the environment variable is set.
//!
//! Evidence tasks use it as:
//! `HANG_CDP_PORT=9222` + debug binary + `pnpm dev` → `http://127.0.0.1:9222/json/version`.

/// Browser arguments that expose the WebView2 DevTools protocol, when this is a debug build and
/// `HANG_CDP_PORT` holds a usable port.
pub fn cdp_browser_args() -> Option<String> {
    #[cfg(debug_assertions)]
    {
        parse_port(std::env::var("HANG_CDP_PORT").ok().as_deref()).map(browser_args)
    }
    #[cfg(not(debug_assertions))]
    {
        // Release safety: the hook is compiled out; the env var is deliberately ignored.
        None
    }
}

/// wry 0.57's default browser arguments (it passes these when the app does not override them).
///
/// Evidence runs must keep the same WebView2 feature set as release builds, so the CDP switch is
/// *prepended* to these defaults instead of replacing them.
#[cfg(any(debug_assertions, test))]
const DEFAULT_BROWSER_ARGS: &str =
    "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection";

/// Full additional-browser-arguments string for a debug evidence run.
#[cfg(any(debug_assertions, test))]
fn browser_args(port: u16) -> String {
    format!("{DEFAULT_BROWSER_ARGS} --remote-debugging-port={port}")
}

/// Parses a DevTools port: a non-zero `u16`, tolerant of surrounding whitespace.
///
/// Only ASCII digits are accepted (Rust's `u16` parser would also accept a leading `+`; the port
/// environment variable should be a plain decimal number).
#[cfg(any(debug_assertions, test))]
fn parse_port(raw: Option<&str>) -> Option<u16> {
    let text = raw?.trim();
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let port = text.parse::<u16>().ok()?;
    (port > 0).then_some(port)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_usable_ports() {
        assert_eq!(parse_port(Some("9222")), Some(9222));
        assert_eq!(parse_port(Some("1")), Some(1));
        assert_eq!(parse_port(Some("65535")), Some(65535));
        // Environment variables commonly carry surrounding whitespace on Windows.
        assert_eq!(parse_port(Some("  9222\n")), Some(9222));
    }

    #[test]
    fn rejects_missing_empty_and_invalid_values() {
        assert_eq!(parse_port(None), None);
        assert_eq!(parse_port(Some("")), None);
        assert_eq!(parse_port(Some("   ")), None);
        assert_eq!(parse_port(Some("abc")), None);
        assert_eq!(parse_port(Some("92 22")), None);
        assert_eq!(parse_port(Some("-1")), None);
        assert_eq!(parse_port(Some("+9222")), None);
        assert_eq!(parse_port(Some("0x2416")), None);
    }

    #[test]
    fn rejects_out_of_range_and_zero() {
        // Port 0 would make the OS pick an unpredictable port, so evidence could not attach.
        assert_eq!(parse_port(Some("0")), None);
        assert_eq!(parse_port(Some("65536")), None);
        assert_eq!(parse_port(Some("99999")), None);
    }

    #[test]
    fn browser_args_keep_wry_defaults_and_append_the_port() {
        let args = browser_args(9222);
        assert!(args.starts_with(DEFAULT_BROWSER_ARGS), "defaults must be prepended: {args}");
        assert!(args.contains("--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection"));
        assert!(args.ends_with("--remote-debugging-port=9222"));
        assert_eq!(args.matches("--remote-debugging-port=").count(), 1);
    }
}
