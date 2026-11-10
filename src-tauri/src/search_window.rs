//! On-demand search window (Wave 2 W2.2).
//!
//! The window is created only when the user asks for it (tray "Search…", the `search` global
//! shortcut or the panel) and is **destroyed on close**, never hidden. That keeps the resident
//! memory baseline flat: Wave 0 measured the app's idle footprint precisely because every window
//! costs tens of MB, and a pre-warmed search webview would spend it permanently.
//!
//! [`open`] is idempotent: an already-open search window is brought to the foreground instead of
//! being duplicated. [`open_in_background`] is the event-loop-safe wrapper for the global-shortcut
//! and tray handlers (window creation dispatches to the loop and waits, so it must not run there).
//!
//! Logs pair `search: window opened` with `search: window closed` so the memory evidence can show
//! that closing really releases the webview.

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

use crate::app_state::AppState;
use crate::i18n;
use crate::settings::Theme;

/// Label of the search webview (`index.html?window=search`).
pub const LABEL: &str = "search";

/// Default logical size (W2.2: about 860×560).
const WIDTH_DP: f64 = 860.0;
/// Default logical height.
const HEIGHT_DP: f64 = 560.0;
/// Minimum logical width (W2.2: 560×360) — the two-column result list stops being usable below it.
const MIN_WIDTH_DP: f64 = 560.0;
/// Minimum logical height.
const MIN_HEIGHT_DP: f64 = 360.0;

/// URL/query of the search page (P2-8 theme + Wave 2 language contract; unit-tested).
///
/// The page resolves `theme=system` itself against `prefers-color-scheme`; `lang` is the resolved
/// Rust language tag (`"en"`/`"zh-Hans"`) so the search window cannot disagree with the panel.
pub(crate) fn search_url(theme: Theme, lang: &str) -> String {
    format!(
        "index.html?window=search&theme={}&lang={lang}",
        theme.as_str()
    )
}

/// Opens (or re-focuses) the on-demand search window.
///
/// Must run off the event-loop thread when called from a global-shortcut/menu handler
/// ([`open_in_background`] is that wrapper). Closing the window destroys it (Tauri's default), so
/// the next call recreates a fresh page instead of reusing a hidden, memory-holding webview.
pub fn open(app: &AppHandle) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(LABEL) {
        log::info!("search: window already open; focusing it");
        window.unminimize().ok();
        window.show().ok();
        window.set_focus().ok();
        return Ok(());
    }

    let (theme, lang) = app
        .try_state::<AppState>()
        .map(|state| {
            let snapshot = state.snapshot();
            (snapshot.settings.theme, snapshot.resolved_language)
        })
        .unwrap_or((Theme::System, "en".to_string()));
    let strings = i18n::tray_strings(i18n::app_language(app));
    let mut builder =
        WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App(search_url(theme, &lang).into()))
            .title(strings.search_window_title)
            .inner_size(WIDTH_DP, HEIGHT_DP)
            .min_inner_size(MIN_WIDTH_DP, MIN_HEIGHT_DP)
            .resizable(true)
            .center()
            .focused(true)
            .visible(true);
    // Debug-only runtime-evidence hook; no-op in release builds (see `devtools`).
    if let Some(args) = crate::devtools::cdp_browser_args() {
        builder = builder.additional_browser_args(&args);
    }
    let window = builder.build()?;
    // Close = destroy (never hide): the log line below is the closing half of the memory evidence.
    window.on_window_event(|event| {
        if matches!(event, tauri::WindowEvent::Destroyed) {
            log::info!("search: window closed (destroyed; resident webview memory released)");
        }
    });
    let _ = window.set_focus();
    log::info!(
        "search: window opened ({WIDTH_DP}x{HEIGHT_DP} dp, min {MIN_WIDTH_DP}x{MIN_HEIGHT_DP} dp, \
         label {LABEL})"
    );
    Ok(())
}

/// Opens the search window on a named worker thread (global-shortcut / tray / event-loop callers).
pub fn open_in_background(app: &AppHandle) {
    let app = app.clone();
    let spawned = std::thread::Builder::new()
        .name("hangline-search".to_string())
        .spawn(move || {
            if let Err(error) = open(&app) {
                log::warn!("search: cannot open the search window: {error}");
            }
        });
    if let Err(error) = spawned {
        log::error!("search: cannot spawn the search window worker thread: {error}");
    }
}

/// Closes (and therefore destroys) the search window; a missing window is a logged no-op.
pub fn close(app: &AppHandle) {
    let Some(window) = app.get_webview_window(LABEL) else {
        log::info!("search: close requested with no open search window");
        return;
    };
    match window.close() {
        Ok(()) => log::info!("search: window closed by request"),
        Err(error) => log::warn!("search: cannot close the search window: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The search URL is the frontend contract: the window kind, every theme and the resolved
    /// language must be carried explicitly (a missing/renamed theme silently degrades to system).
    #[test]
    fn window_url_carries_the_window_kind_theme_and_language() {
        for (theme, wire) in [
            (Theme::System, "system"),
            (Theme::Light, "light"),
            (Theme::Dark, "dark"),
        ] {
            let url = search_url(theme, "zh-Hans");
            assert!(url.starts_with("index.html?"), "url: {url}");
            assert!(url.contains("window=search"), "url: {url}");
            assert!(url.contains(&format!("theme={wire}")), "url: {url}");
            assert!(url.contains("lang=zh-Hans"), "url: {url}");
        }
        assert_eq!(LABEL, "search");
        assert_eq!((WIDTH_DP, HEIGHT_DP), (860.0, 560.0));
        assert_eq!((MIN_WIDTH_DP, MIN_HEIGHT_DP), (560.0, 360.0));
        assert!(
            MIN_WIDTH_DP < WIDTH_DP && MIN_HEIGHT_DP < HEIGHT_DP,
            "the default size must be larger than the minimum"
        );
    }
}
