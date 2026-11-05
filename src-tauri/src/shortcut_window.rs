//! The minimal "change shortcut" window (`?window=shortcut`, design spec §4.2 / Task 15).
//!
//! Opened from the tray's Shortcut submenu. The window is small, centered, always on top and
//! focusable (it is the only Hangline window the user is expected to type into). The frontend
//! captures the key combination, validates it against the same accelerator grammar as
//! [`crate::hotkeys::normalize`], and saves it through the shared settings path (`set_hotkey` →
//! `actions::apply`), which re-registers the global shortcut, persists it and rolls back on a
//! conflict.
//!
//! One window is reused: opening it again for a different target focuses the existing window and
//! emits `shortcut-target` so the frontend switches which shortcut it edits.

use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

use crate::app_state::AppState;
use crate::hotkeys::Target;
use crate::i18n;
use crate::settings::Theme;

/// Label of the shortcut window (`get_webview_window`/event targeting).
pub const LABEL: &str = "shortcut";

/// Fixed logical size: wide enough for the captured combination and the error line.
const WIDTH_DP: f64 = 460.0;
/// Fixed logical height (capture box + two buttons).
const HEIGHT_DP: f64 = 240.0;

/// URL/query of the change-shortcut page (P2-8 theme contract; unit-tested).
pub(crate) fn shortcut_url(target: &str, theme: Theme) -> String {
    format!(
        "index.html?window=shortcut&target={target}&theme={}",
        theme.as_str()
    )
}

/// Opens (or re-focuses) the change-shortcut window for `target`.
pub fn open(app: &AppHandle, target: Target) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(LABEL) {
        log::info!(
            "shortcut: window already open; switching it to the {} target",
            target.as_str()
        );
        if let Err(error) = window.emit("shortcut-target", target.as_str()) {
            log::warn!("shortcut: failed to emit shortcut-target: {error}");
        }
        window.unminimize().ok();
        window.show().ok();
        window.set_focus().ok();
        return Ok(());
    }

    // P2-8: pass the raw theme setting so the window can theme its glass/chips without an app store.
    let theme = app.state::<AppState>().snapshot().settings.theme;
    let url = shortcut_url(target.as_str(), theme);
    let strings = i18n::tray_strings(i18n::app_language(app));
    let mut builder = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App(url.into()))
        .title(strings.shortcut_window_title)
        .inner_size(WIDTH_DP, HEIGHT_DP)
        .resizable(false)
        .always_on_top(true)
        .center()
        .focused(true)
        .visible(true);
    // Debug-only runtime-evidence hook; no-op in release builds (see `devtools`).
    if let Some(args) = crate::devtools::cdp_browser_args() {
        builder = builder.additional_browser_args(&args);
    }
    let window = builder.build()?;
    // The panel is never focusable, so this window is what the user interacts with; it must be
    // closable with the usual title-bar close button and must not keep the app alive on its own.
    log::info!(
        "shortcut: change-shortcut window opened for the {} target",
        target.as_str()
    );
    let _ = window.set_focus();
    Ok(())
}

/// Closes the change-shortcut window (Esc/Cancel/Save in the frontend).
pub fn close(app: &AppHandle) {
    let Some(window) = app.get_webview_window(LABEL) else {
        return;
    };
    if let Err(error) = window.close() {
        log::warn!("shortcut: failed to close the window: {error}");
    } else {
        log::info!("shortcut: change-shortcut window closed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The query-string form is the frontend contract; both targets and every theme must be
    /// carried explicitly (a missing/renamed theme silently degrades to system).
    #[test]
    fn window_url_carries_the_target_and_theme() {
        for target in [Target::Capture, Target::Toggle] {
            for (theme, wire) in [
                (Theme::System, "system"),
                (Theme::Light, "light"),
                (Theme::Dark, "dark"),
            ] {
                let url = shortcut_url(target.as_str(), theme);
                assert!(url.starts_with("index.html?"), "url: {url}");
                assert!(url.contains("window=shortcut"), "url: {url}");
                assert!(url.contains(&format!("target={}", target.as_str())), "url: {url}");
                assert!(url.contains(&format!("theme={wire}")), "url: {url}");
            }
        }
        assert_eq!(LABEL, "shortcut");
        assert!(WIDTH_DP > HEIGHT_DP);
    }
}
