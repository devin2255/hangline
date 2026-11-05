//! System tray: the complete menu tree from design spec §3.5, kept in sync with the settings.
//!
//! The tray is one window-less icon with a native menu whose labels/checkmarks mirror the persisted
//! state. The menu is rebuilt only when the effective UI language changes (every label differs);
//! every other state change updates the existing item handles in place, so an open menu is not
//! closed by a new screenshot.
//!
//! # Threading
//!
//! Tauri delivers menu events on the main thread. A menu action therefore never blocks it: the
//! mutating actions (settings writes + watcher restarts, take-everything-down, Quit, folder picker,
//! ShellExecute) are moved to a named background thread. The menu refresh itself is marshalled to
//! the main thread with [`AppHandle::run_on_main_thread`] (Tauri runs the closure directly when it
//! is already on the main thread), because the native menu/item APIs are main-thread owned.
//!
//! # State model
//!
//! [`menu_state`] is a pure function of the settings snapshot + panel visibility + item count +
//! hotkey failure + real autostart registration; [`MenuState`] is what the refresh compares to skip
//! redundant native updates. The action mapping ([`action_for`]) is pure as well, so both halves are
//! unit-tested without a Tauri runtime.

use std::sync::Mutex;

use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Wry};

use crate::actions;
use crate::app_state::AppState;
use crate::hotkeys::{self, HotkeyFailure, Target};
use crate::i18n::{self, Lang, TrayStrings};
use crate::panel;
use crate::reveal::{self, RevealReason};
use crate::settings::{CardSize, KeepForDays, KeepOnLine, Settings, MAX_EXTRA_FOLDERS};

/// Stable tray icon id (looked up by tests/`tray_by_id`).
pub const TRAY_ID: &str = "hangline-tray";

/// Show/hide line (the label changes with the panel state).
pub const ID_TOGGLE: &str = "tray.toggle";
/// "Take everything down".
pub const ID_TAKE_ALL_DOWN: &str = "tray.take_all_down";
/// "Take screenshot…" (Task 16 replaces the temporary capture request).
pub const ID_CAPTURE: &str = "tray.capture";
/// "Handle system screenshots" switch (`watchSystemScreenshots`).
pub const ID_HANDLE_SCREENSHOTS: &str = "tray.handle_screenshots";
/// "Hang copied images" switch (`watchClipboard`).
pub const ID_HANG_COPIED: &str = "tray.hang_copied";
/// "Handle recordings" switch (`watchRecordings`).
pub const ID_HANDLE_RECORDINGS: &str = "tray.handle_recordings";
/// Reveal-on-new-screenshot switch (`revealOnNew`).
pub const ID_REVEAL_ON_NEW: &str = "tray.reveal_on_new";
/// Top-edge-reveal switch (`topEdgeReveal`).
pub const ID_TOP_EDGE_REVEAL: &str = "tray.top_edge_reveal";
/// Show-over-full-screen switch (`showOverFullScreen`).
pub const ID_OVER_FULL_SCREEN: &str = "tray.show_over_full_screen";
/// "Add Folder…" action.
pub const ID_ADD_FOLDER: &str = "tray.folder.add";
/// "Keep on line ▸ As many as fit" (`keepOnLine = "fit"`).
pub const ID_KEEP_FIT: &str = "tray.keep.fit";
/// "Keep on line ▸ 25".
pub const ID_KEEP_25: &str = "tray.keep.25";
/// "Keep on line ▸ 50".
pub const ID_KEEP_50: &str = "tray.keep.50";
/// "Keep on line ▸ 100".
pub const ID_KEEP_100: &str = "tray.keep.100";
/// "Keep for ▸ Keep forever" (`keepForDays = "forever"`).
pub const ID_KEEP_FOREVER: &str = "tray.keep_for.forever";
/// "Keep for ▸ Keep last 7 days".
pub const ID_KEEP_FOR_7: &str = "tray.keep_for.7";
/// "Keep for ▸ Keep last 30 days".
pub const ID_KEEP_FOR_30: &str = "tray.keep_for.30";
/// "Keep for ▸ Keep last 90 days".
pub const ID_KEEP_FOR_90: &str = "tray.keep_for.90";
/// "Size ▸ Small".
pub const ID_SIZE_SMALL: &str = "tray.size.small";
/// "Size ▸ Medium".
pub const ID_SIZE_MEDIUM: &str = "tray.size.medium";
/// "Size ▸ Large".
pub const ID_SIZE_LARGE: &str = "tray.size.large";
/// "Shortcut ▸ Capture: <accel>" row (opens the change window for capture).
pub const ID_HOTKEY_CAPTURE: &str = "tray.hotkey.capture";
/// "Shortcut ▸ Show/hide: <accel>" row (opens the change window for toggle).
pub const ID_HOTKEY_TOGGLE: &str = "tray.hotkey.toggle";
/// "Shortcut ▸ <status>" disabled hint row (shows a registration failure).
pub const ID_HOTKEY_STATUS: &str = "tray.hotkey.status";
/// "Open screenshots folder".
pub const ID_OPEN_SCREENSHOTS: &str = "tray.open_screenshots";
/// "Sounds" switch (`sound`).
pub const ID_SOUND: &str = "tray.sound";
/// "Open at login" switch (`startAtLogin`, mirrors the real registration).
pub const ID_START_AT_LOGIN: &str = "tray.start_at_login";
/// "Check for updates…".
pub const ID_CHECK_UPDATES: &str = "tray.check_updates";
/// "Language ▸ Follow system".
pub const ID_LANGUAGE_SYSTEM: &str = "tray.language.system";
/// "Language ▸ English".
pub const ID_LANGUAGE_EN: &str = "tray.language.en";
/// "Language ▸ 简体中文".
pub const ID_LANGUAGE_ZH: &str = "tray.language.zh-Hans";
/// "Quit Hangline".
pub const ID_QUIT: &str = "tray.quit";

/// Prefix of a watched-folder row (click = open it in Explorer).
const FOLDER_OPEN_PREFIX: &str = "tray.folder.open.";
/// Prefix of a "Stop watching ▸" row (click = remove it from `extraFolders`).
const FOLDER_STOP_PREFIX: &str = "tray.folder.stop.";

/// One tray menu action (the mapping from menu id is [`action_for`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrayAction {
    /// Show or hide the line (pointer-source toggle).
    ToggleLine,
    /// Take every card off the line (own-dir files recycled, watched-folder files kept).
    TakeEverythingDown,
    /// Request a region screenshot (temporary wiring until Task 16).
    TakeScreenshot,
    /// Toggle `watchSystemScreenshots` (restarts the folder watchers).
    ToggleWatchSystemScreenshots,
    /// Toggle `watchRecordings` (restarts the folder watchers).
    ToggleWatchRecordings,
    /// Toggle `watchClipboard` (starts/stops the clipboard listener).
    ToggleWatchClipboard,
    /// Toggle `revealOnNew`.
    ToggleRevealOnNew,
    /// Toggle `topEdgeReveal`.
    ToggleTopEdgeReveal,
    /// Toggle `showOverFullScreen`.
    ToggleShowOverFullScreen,
    /// Open the extra watched folder at `index` in Explorer.
    OpenWatchedFolder(usize),
    /// Show the system folder picker and add the chosen folder.
    AddFolder,
    /// Remove the extra watched folder at `index`.
    StopWatching(usize),
    /// Set `keepOnLine`.
    SetKeepOnLine(KeepOnLine),
    /// Set `keepForDays` (age retention; `forever` or 7/30/90 days).
    SetKeepForDays(KeepForDays),
    /// Set `cardSize`.
    SetCardSize(CardSize),
    /// Show the change-shortcut window for a target.
    ChangeShortcut(Target),
    /// Open Hangline's own screenshot directory.
    OpenScreenshotsFolder,
    /// Toggle `sound`.
    ToggleSound,
    /// Toggle `startAtLogin` (registers/unregisters the login item).
    ToggleStartAtLogin,
    /// Open the (placeholder) Releases page in the browser.
    CheckForUpdates,
    /// Set `language`; `None` = follow the system.
    SetLanguage(Option<Lang>),
    /// Stop the watchers/clipboard and exit.
    Quit,
}

impl TrayAction {
    /// Short stable name for logs (`tray: menu action …`).
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ToggleLine => "toggle line",
            Self::TakeEverythingDown => "take everything down",
            Self::TakeScreenshot => "take screenshot",
            Self::ToggleWatchSystemScreenshots => "toggle system screenshots",
            Self::ToggleWatchRecordings => "toggle recordings",
            Self::ToggleWatchClipboard => "toggle clipboard",
            Self::ToggleRevealOnNew => "toggle reveal on new",
            Self::ToggleTopEdgeReveal => "toggle top edge reveal",
            Self::ToggleShowOverFullScreen => "toggle show over full screen",
            Self::OpenWatchedFolder(_) => "open watched folder",
            Self::AddFolder => "add folder",
            Self::StopWatching(_) => "stop watching",
            Self::SetKeepOnLine(_) => "set keep on line",
            Self::SetKeepForDays(_) => "set keep for days",
            Self::SetCardSize(_) => "set card size",
            Self::ChangeShortcut(_) => "change shortcut",
            Self::OpenScreenshotsFolder => "open screenshots folder",
            Self::ToggleSound => "toggle sound",
            Self::ToggleStartAtLogin => "toggle start at login",
            Self::CheckForUpdates => "check for updates",
            Self::SetLanguage(_) => "set language",
            Self::Quit => "quit",
        }
    }
}

/// Dynamic menu id of the "open watched folder `index`" row.
pub fn folder_open_id(index: usize) -> String {
    format!("{FOLDER_OPEN_PREFIX}{index}")
}

/// Dynamic menu id of the "stop watching `index`" row.
pub fn folder_stop_id(index: usize) -> String {
    format!("{FOLDER_STOP_PREFIX}{index}")
}

/// Maps a menu item id to its action (pure; unit-tested).
///
/// Dynamic folder ids are parsed back to their index; an unknown id (a stale row after the folder
/// list changed) is a logged no-op instead of a panic.
pub fn action_for(id: &str) -> Option<TrayAction> {
    let action = match id {
        ID_TOGGLE => TrayAction::ToggleLine,
        ID_TAKE_ALL_DOWN => TrayAction::TakeEverythingDown,
        ID_CAPTURE => TrayAction::TakeScreenshot,
        ID_HANDLE_SCREENSHOTS => TrayAction::ToggleWatchSystemScreenshots,
        ID_HANG_COPIED => TrayAction::ToggleWatchClipboard,
        ID_HANDLE_RECORDINGS => TrayAction::ToggleWatchRecordings,
        ID_REVEAL_ON_NEW => TrayAction::ToggleRevealOnNew,
        ID_TOP_EDGE_REVEAL => TrayAction::ToggleTopEdgeReveal,
        ID_OVER_FULL_SCREEN => TrayAction::ToggleShowOverFullScreen,
        ID_ADD_FOLDER => TrayAction::AddFolder,
        ID_KEEP_FIT => TrayAction::SetKeepOnLine(KeepOnLine::Fit),
        ID_KEEP_25 => TrayAction::SetKeepOnLine(KeepOnLine::Limit(25)),
        ID_KEEP_50 => TrayAction::SetKeepOnLine(KeepOnLine::Limit(50)),
        ID_KEEP_100 => TrayAction::SetKeepOnLine(KeepOnLine::Limit(100)),
        ID_KEEP_FOREVER => TrayAction::SetKeepForDays(KeepForDays::Forever),
        ID_KEEP_FOR_7 => TrayAction::SetKeepForDays(KeepForDays::Days(7)),
        ID_KEEP_FOR_30 => TrayAction::SetKeepForDays(KeepForDays::Days(30)),
        ID_KEEP_FOR_90 => TrayAction::SetKeepForDays(KeepForDays::Days(90)),
        ID_SIZE_SMALL => TrayAction::SetCardSize(CardSize::Small),
        ID_SIZE_MEDIUM => TrayAction::SetCardSize(CardSize::Medium),
        ID_SIZE_LARGE => TrayAction::SetCardSize(CardSize::Large),
        ID_HOTKEY_CAPTURE => TrayAction::ChangeShortcut(Target::Capture),
        ID_HOTKEY_TOGGLE => TrayAction::ChangeShortcut(Target::Toggle),
        ID_OPEN_SCREENSHOTS => TrayAction::OpenScreenshotsFolder,
        ID_SOUND => TrayAction::ToggleSound,
        ID_START_AT_LOGIN => TrayAction::ToggleStartAtLogin,
        ID_CHECK_UPDATES => TrayAction::CheckForUpdates,
        ID_LANGUAGE_SYSTEM => TrayAction::SetLanguage(None),
        ID_LANGUAGE_EN => TrayAction::SetLanguage(Some(Lang::En)),
        ID_LANGUAGE_ZH => TrayAction::SetLanguage(Some(Lang::ZhHans)),
        ID_QUIT => TrayAction::Quit,
        _ => return folder_action(id),
    };
    Some(action)
}

/// Parses a dynamic `tray.folder.open.<n>` / `tray.folder.stop.<n>` id.
fn folder_action(id: &str) -> Option<TrayAction> {
    if let Some(index) = id.strip_prefix(FOLDER_OPEN_PREFIX) {
        return index.parse().ok().map(TrayAction::OpenWatchedFolder);
    }
    if let Some(index) = id.strip_prefix(FOLDER_STOP_PREFIX) {
        return index.parse().ok().map(TrayAction::StopWatching);
    }
    None
}

/// The dynamic half of the tray menu: everything [`refresh`] writes into the native items.
///
/// Built by the pure [`menu_state`] function and compared by the refresh to skip redundant native
/// updates (the full state is broadcast on every screenshot, most of which leave the menu intact).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuState {
    /// Effective UI language (labels are chosen from it).
    pub lang: Lang,
    /// `show_line`/`hide_line` label for the current panel visibility.
    pub toggle_label: &'static str,
    /// Whether the line's own screenshot accelerator is shown.
    pub capture_hotkey: String,
    /// Whether the line's show/hide accelerator is shown.
    pub toggle_hotkey: String,
    /// "Take everything down" is enabled only when something is on the line.
    pub take_everything_down_enabled: bool,
    /// Whether the system screenshot folder is watched.
    pub handle_screenshots: bool,
    /// Whether the clipboard listener is on.
    pub hang_copied_images: bool,
    /// Whether the recordings folder is watched.
    pub handle_recordings: bool,
    /// Reveal on new items.
    pub reveal_on_new: bool,
    /// Reveal by resting the pointer on the top edge.
    pub top_edge_reveal: bool,
    /// Keep the line over full-screen apps.
    pub show_over_full_screen: bool,
    /// The extra watched folders (also the submenu contents).
    pub extra_folders: Vec<String>,
    /// Whether another extra folder may be added.
    pub add_folder_enabled: bool,
    /// Kept-on-line limit.
    pub keep_on_line: KeepOnLine,
    /// Age-retention policy.
    pub keep_for_days: KeepForDays,
    /// Card size.
    pub card_size: CardSize,
    /// First hotkey registration failure, shown as a disabled hint row + tooltip suffix.
    pub hotkey_error: Option<HotkeyFailure>,
    /// Copy/discard sounds.
    pub sound: bool,
    /// Real login-item registration (not just the persisted setting).
    pub start_at_login: bool,
    /// Persisted `language` value (`"system"` or a tag).
    pub language: String,
    /// Items currently on the line (tooltip count).
    pub item_count: usize,
}

/// Builds the pure [`MenuState`] for the tray (unit-tested; no Tauri handle involved).
pub fn menu_state(
    settings: &Settings,
    visible: bool,
    item_count: usize,
    hotkey_error: Option<HotkeyFailure>,
    start_at_login: bool,
    lang: Lang,
) -> MenuState {
    let strings = i18n::tray_strings(lang);
    MenuState {
        lang,
        toggle_label: if visible {
            strings.hide_line
        } else {
            strings.show_line
        },
        capture_hotkey: settings.capture_hotkey.clone(),
        toggle_hotkey: settings.toggle_hotkey.clone(),
        take_everything_down_enabled: item_count > 0,
        handle_screenshots: settings.watch_system_screenshots,
        hang_copied_images: settings.watch_clipboard,
        handle_recordings: settings.watch_recordings,
        reveal_on_new: settings.reveal_on_new,
        top_edge_reveal: settings.top_edge_reveal,
        show_over_full_screen: settings.show_over_full_screen,
        extra_folders: settings.extra_folders.clone(),
        add_folder_enabled: settings.extra_folders.len() < MAX_EXTRA_FOLDERS,
        keep_on_line: settings.keep_on_line,
        keep_for_days: settings.keep_for_days,
        card_size: settings.card_size,
        hotkey_error,
        sound: settings.sound,
        start_at_login,
        language: settings.language.clone(),
        item_count,
    }
}

/// Tray tooltip: item count (design spec §3.11 "线上 0 张") plus a shortcut failure suffix.
pub fn tooltip(strings: &TrayStrings, state: &MenuState) -> String {
    let base = strings
        .tooltip_items
        .replacen("{}", &state.item_count.to_string(), 1);
    if state.hotkey_error.is_some() {
        format!("{base}{}", strings.tooltip_hotkey_unavailable)
    } else {
        base
    }
}

/// Tooltip text for the shortcut status row (disabled item, no balloon API available).
fn hotkey_status_text(strings: &TrayStrings, state: &MenuState) -> String {
    match &state.hotkey_error {
        None => strings.hotkeys_ok.to_string(),
        Some(failure) => {
            let target = match failure.target {
                Target::Capture => strings.shortcut_capture,
                Target::Toggle => strings.shortcut_toggle,
            };
            format!(
                "⚠ {}: {target} — {}",
                strings.hotkey_unavailable, failure.message
            )
        }
    }
}

/// Label of one shortcut row: `Capture: Ctrl+Alt+S ⚠` when its registration failed.
fn hotkey_row_label(label: &str, accelerator: &str, failed: bool) -> String {
    if failed {
        format!("{label}: {accelerator} ⚠")
    } else {
        format!("{label}: {accelerator}")
    }
}

/// The native menu item handles the refresh updates in place.
struct Items {
    toggle: MenuItem<Wry>,
    take_all_down: MenuItem<Wry>,
    capture: MenuItem<Wry>,
    handle_screenshots: CheckMenuItem<Wry>,
    hang_copied: CheckMenuItem<Wry>,
    handle_recordings: CheckMenuItem<Wry>,
    reveal_on_new: CheckMenuItem<Wry>,
    top_edge_reveal: CheckMenuItem<Wry>,
    over_full_screen: CheckMenuItem<Wry>,
    folders_menu: Submenu<Wry>,
    add_folder: MenuItem<Wry>,
    stop_watching_menu: Submenu<Wry>,
    keep_fit: CheckMenuItem<Wry>,
    keep_25: CheckMenuItem<Wry>,
    keep_50: CheckMenuItem<Wry>,
    keep_100: CheckMenuItem<Wry>,
    keep_forever: CheckMenuItem<Wry>,
    keep_for_7: CheckMenuItem<Wry>,
    keep_for_30: CheckMenuItem<Wry>,
    keep_for_90: CheckMenuItem<Wry>,
    size_small: CheckMenuItem<Wry>,
    size_medium: CheckMenuItem<Wry>,
    size_large: CheckMenuItem<Wry>,
    hotkey_capture: MenuItem<Wry>,
    hotkey_toggle: MenuItem<Wry>,
    hotkey_status: MenuItem<Wry>,
    open_screenshots: MenuItem<Wry>,
    sound: CheckMenuItem<Wry>,
    start_at_login: CheckMenuItem<Wry>,
    check_updates: MenuItem<Wry>,
    language_system: CheckMenuItem<Wry>,
    language_en: CheckMenuItem<Wry>,
    language_zh: CheckMenuItem<Wry>,
    quit: MenuItem<Wry>,
}

/// Managed tray handles (the icon and the current menu items).
struct TrayHandles {
    tray: TrayIcon<Wry>,
    lang: Lang,
    items: Items,
    /// Last applied state; an unchanged state skips all native updates.
    last: MenuState,
}

/// Managed state wrapper (Tauri requires a unique type per managed value).
pub struct TrayHandleState(Mutex<Option<TrayHandles>>);

/// Creates the tray icon and the full menu. Called once from `setup` after `hotkeys::init`.
pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let Some(snapshot) = app.try_state::<AppState>().map(|state| state.snapshot()) else {
        log::warn!("tray: app state is not managed; the tray menu will not be created");
        return Ok(());
    };
    let lang = i18n::app_language(app);
    let state = menu_state(
        &snapshot.settings,
        panel::is_visible(),
        snapshot.items.len(),
        hotkeys::failure(app),
        autostart_enabled(app, &snapshot.settings),
        lang,
    );
    let strings = i18n::tray_strings(lang);
    let (menu, items) = build_menu(app, strings, &state)?;

    let builder = TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        // Right click opens the menu; left click is the quick show/hide toggle.
        .show_menu_on_left_click(false)
        .tooltip(tooltip(strings, &state))
        .on_menu_event(|app, event| handle_menu(app, event.id.as_ref()))
        .on_tray_icon_event(|tray, event| handle_icon_event(tray.app_handle(), &event));
    let builder = match app.default_window_icon() {
        Some(icon) => builder.icon(icon.clone()),
        None => {
            log::warn!("tray: no application icon available; the tray icon will be blank");
            builder
        }
    };
    let tray = builder.build(app)?;

    app.manage(TrayHandleState(Mutex::new(Some(TrayHandles {
        tray,
        lang,
        items,
        last: state,
    }))));
    log::info!("tray: icon and full menu created ({} language)", lang.tag());
    Ok(())
}

/// Schedules a tray refresh on the main thread (safe to call from any thread).
///
/// Called by the state broadcast choke point and by `panel::show`/`hide`; a menu change can be
/// triggered from a watcher thread, and the native menu APIs must run on the main thread. Tauri
/// executes the closure directly when the caller is already on the main thread.
pub fn sync_state(app: &AppHandle) {
    let handle = app.clone();
    if let Err(error) = app.run_on_main_thread(move || refresh(&handle)) {
        log::warn!("tray: cannot schedule a menu refresh: {error}");
    }
}

/// Forces a menu refresh now (language change; equivalent to [`sync_state`] but named for intent).
pub fn rebuild(app: &AppHandle) {
    sync_state(app);
}

/// Applies the current state to the native menu (main thread).
fn refresh(app: &AppHandle) {
    let Some(handle_state) = app.try_state::<TrayHandleState>() else {
        return;
    };
    let Some(snapshot) = app.try_state::<AppState>().map(|state| state.snapshot()) else {
        return;
    };
    let lang = i18n::app_language(app);
    let state = menu_state(
        &snapshot.settings,
        panel::is_visible(),
        snapshot.items.len(),
        hotkeys::failure(app),
        autostart_enabled(app, &snapshot.settings),
        lang,
    );

    let mut guard = handle_state
        .0
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(handles) = guard.as_mut() else {
        return;
    };
    if handles.last == state {
        return;
    }
    let strings = i18n::tray_strings(state.lang);

    if handles.lang != state.lang {
        // Every label changes with the language: rebuild the whole tree and swap it atomically.
        match build_menu(app, strings, &state) {
            Ok((menu, items)) => match handles.tray.set_menu(Some(menu)) {
                Ok(()) => {
                    handles.items = items;
                    handles.lang = state.lang;
                    log::info!("tray: menu rebuilt for language {}", state.lang.tag());
                }
                Err(error) => {
                    log::warn!("tray: cannot swap the rebuilt menu: {error}");
                    return;
                }
            },
            Err(error) => {
                log::warn!("tray: cannot rebuild the menu: {error}");
                return;
            }
        }
    }

    if let Err(error) = apply_state(app, strings, &handles.items, &state) {
        log::warn!("tray: cannot apply the menu state: {error}");
        return;
    }
    if let Err(error) = handles.tray.set_tooltip(Some(tooltip(strings, &state))) {
        log::warn!("tray: cannot update the tray tooltip: {error}");
    }
    handles.last = state;
}

/// Creates every menu item/submenu for one state and returns the assembled menu.
fn build_menu(
    app: &AppHandle,
    strings: &TrayStrings,
    state: &MenuState,
) -> tauri::Result<(Menu<Wry>, Items)> {
    let toggle = MenuItem::with_id(
        app,
        ID_TOGGLE,
        state.toggle_label,
        true,
        Some(state.toggle_hotkey.as_str()),
    )?;
    let take_all_down =
        MenuItem::with_id(app, ID_TAKE_ALL_DOWN, strings.take_everything_down, state.take_everything_down_enabled, None::<&str>)?;
    let capture = MenuItem::with_id(
        app,
        ID_CAPTURE,
        strings.capture,
        true,
        Some(state.capture_hotkey.as_str()),
    )?;

    let handle_screenshots = CheckMenuItem::with_id(
        app,
        ID_HANDLE_SCREENSHOTS,
        strings.handle_screenshots,
        true,
        state.handle_screenshots,
        None::<&str>,
    )?;
    let hang_copied = CheckMenuItem::with_id(
        app,
        ID_HANG_COPIED,
        strings.hang_copied_images,
        true,
        state.hang_copied_images,
        None::<&str>,
    )?;
    let handle_recordings = CheckMenuItem::with_id(
        app,
        ID_HANDLE_RECORDINGS,
        strings.handle_recordings,
        true,
        state.handle_recordings,
        None::<&str>,
    )?;

    let reveal_on_new = CheckMenuItem::with_id(
        app,
        ID_REVEAL_ON_NEW,
        strings.reveal_on_new,
        true,
        state.reveal_on_new,
        None::<&str>,
    )?;
    let top_edge_reveal = CheckMenuItem::with_id(
        app,
        ID_TOP_EDGE_REVEAL,
        strings.top_edge_reveal,
        true,
        state.top_edge_reveal,
        None::<&str>,
    )?;
    let over_full_screen = CheckMenuItem::with_id(
        app,
        ID_OVER_FULL_SCREEN,
        strings.show_over_full_screen,
        true,
        state.show_over_full_screen,
        None::<&str>,
    )?;
    let reveal_menu = Submenu::with_items(
        app,
        strings.reveal_menu,
        true,
        &[&reveal_on_new, &top_edge_reveal, &over_full_screen],
    )?;

    let folders_menu = Submenu::new(app, strings.folders_menu, true)?;
    let add_folder = MenuItem::with_id(
        app,
        ID_ADD_FOLDER,
        if state.add_folder_enabled {
            strings.add_folder
        } else {
            strings.add_folder_max
        },
        state.add_folder_enabled,
        None::<&str>,
    )?;
    let stop_watching_menu = Submenu::new(app, strings.stop_watching_menu, !state.extra_folders.is_empty())?;

    let keep_fit = CheckMenuItem::with_id(
        app,
        ID_KEEP_FIT,
        strings.keep_fit,
        true,
        state.keep_on_line == KeepOnLine::Fit,
        None::<&str>,
    )?;
    let keep_25 = CheckMenuItem::with_id(
        app,
        ID_KEEP_25,
        "25",
        true,
        state.keep_on_line == KeepOnLine::Limit(25),
        None::<&str>,
    )?;
    let keep_50 = CheckMenuItem::with_id(
        app,
        ID_KEEP_50,
        "50",
        true,
        state.keep_on_line == KeepOnLine::Limit(50),
        None::<&str>,
    )?;
    let keep_100 = CheckMenuItem::with_id(
        app,
        ID_KEEP_100,
        "100",
        true,
        state.keep_on_line == KeepOnLine::Limit(100),
        None::<&str>,
    )?;
    let keep_menu = Submenu::with_items(
        app,
        strings.keep_menu,
        true,
        &[&keep_fit, &keep_25, &keep_50, &keep_100],
    )?;

    let keep_forever = CheckMenuItem::with_id(
        app,
        ID_KEEP_FOREVER,
        strings.keep_forever,
        true,
        state.keep_for_days == KeepForDays::Forever,
        None::<&str>,
    )?;
    let keep_for_7 = CheckMenuItem::with_id(
        app,
        ID_KEEP_FOR_7,
        strings.keep_for_7,
        true,
        state.keep_for_days == KeepForDays::Days(7),
        None::<&str>,
    )?;
    let keep_for_30 = CheckMenuItem::with_id(
        app,
        ID_KEEP_FOR_30,
        strings.keep_for_30,
        true,
        state.keep_for_days == KeepForDays::Days(30),
        None::<&str>,
    )?;
    let keep_for_90 = CheckMenuItem::with_id(
        app,
        ID_KEEP_FOR_90,
        strings.keep_for_90,
        true,
        state.keep_for_days == KeepForDays::Days(90),
        None::<&str>,
    )?;
    let keep_for_menu = Submenu::with_items(
        app,
        strings.keep_for_menu,
        true,
        &[&keep_forever, &keep_for_7, &keep_for_30, &keep_for_90],
    )?;

    let size_small = CheckMenuItem::with_id(
        app,
        ID_SIZE_SMALL,
        strings.size_small,
        true,
        state.card_size == CardSize::Small,
        None::<&str>,
    )?;
    let size_medium = CheckMenuItem::with_id(
        app,
        ID_SIZE_MEDIUM,
        strings.size_medium,
        true,
        state.card_size == CardSize::Medium,
        None::<&str>,
    )?;
    let size_large = CheckMenuItem::with_id(
        app,
        ID_SIZE_LARGE,
        strings.size_large,
        true,
        state.card_size == CardSize::Large,
        None::<&str>,
    )?;
    let size_menu = Submenu::with_items(
        app,
        strings.size_menu,
        true,
        &[&size_small, &size_medium, &size_large],
    )?;

    let failed_capture = state
        .hotkey_error
        .as_ref()
        .is_some_and(|failure| failure.target == Target::Capture);
    let failed_toggle = state
        .hotkey_error
        .as_ref()
        .is_some_and(|failure| failure.target == Target::Toggle);
    let hotkey_capture = MenuItem::with_id(
        app,
        ID_HOTKEY_CAPTURE,
        hotkey_row_label(
            strings.shortcut_capture,
            &state.capture_hotkey,
            failed_capture,
        ),
        true,
        None::<&str>,
    )?;
    let hotkey_toggle = MenuItem::with_id(
        app,
        ID_HOTKEY_TOGGLE,
        hotkey_row_label(
            strings.shortcut_toggle,
            &state.toggle_hotkey,
            failed_toggle,
        ),
        true,
        None::<&str>,
    )?;
    let hotkey_status = MenuItem::with_id(
        app,
        ID_HOTKEY_STATUS,
        hotkey_status_text(strings, state),
        false,
        None::<&str>,
    )?;
    let shortcut_menu = Submenu::with_items(
        app,
        strings.shortcut_menu,
        true,
        &[&hotkey_capture, &hotkey_toggle, &PredefinedMenuItem::separator(app)?, &hotkey_status],
    )?;

    let open_screenshots =
        MenuItem::with_id(app, ID_OPEN_SCREENSHOTS, strings.open_screenshots, true, None::<&str>)?;
    let sound = CheckMenuItem::with_id(
        app,
        ID_SOUND,
        strings.sound,
        true,
        state.sound,
        None::<&str>,
    )?;
    let start_at_login = CheckMenuItem::with_id(
        app,
        ID_START_AT_LOGIN,
        strings.start_at_login,
        true,
        state.start_at_login,
        None::<&str>,
    )?;
    let check_updates =
        MenuItem::with_id(app, ID_CHECK_UPDATES, strings.check_updates, true, None::<&str>)?;
    let language_system = CheckMenuItem::with_id(
        app,
        ID_LANGUAGE_SYSTEM,
        strings.language_system,
        true,
        state.language.eq_ignore_ascii_case("system"),
        None::<&str>,
    )?;
    let language_en = CheckMenuItem::with_id(
        app,
        ID_LANGUAGE_EN,
        strings.language_en,
        true,
        state.language.eq_ignore_ascii_case("en"),
        None::<&str>,
    )?;
    let language_zh = CheckMenuItem::with_id(
        app,
        ID_LANGUAGE_ZH,
        strings.language_zh_hans,
        true,
        state.language.eq_ignore_ascii_case("zh-Hans"),
        None::<&str>,
    )?;
    let language_menu = Submenu::with_items(
        app,
        strings.language_menu,
        true,
        &[&language_system, &language_en, &language_zh],
    )?;
    let quit = MenuItem::with_id(app, ID_QUIT, strings.quit, true, None::<&str>)?;

    let items = Items {
        toggle,
        take_all_down,
        capture,
        handle_screenshots,
        hang_copied,
        handle_recordings,
        reveal_on_new,
        top_edge_reveal,
        over_full_screen,
        folders_menu,
        add_folder,
        stop_watching_menu,
        keep_fit,
        keep_25,
        keep_50,
        keep_100,
        keep_forever,
        keep_for_7,
        keep_for_30,
        keep_for_90,
        size_small,
        size_medium,
        size_large,
        hotkey_capture,
        hotkey_toggle,
        hotkey_status,
        open_screenshots,
        sound,
        start_at_login,
        check_updates,
        language_system,
        language_en,
        language_zh,
        quit,
    };
    refresh_folder_items(app, strings, &items, state)?;

    let menu = Menu::with_items(
        app,
        &[
            &items.toggle,
            &items.take_all_down,
            &items.capture,
            &PredefinedMenuItem::separator(app)?,
            &items.handle_screenshots,
            &items.hang_copied,
            &items.handle_recordings,
            &reveal_menu,
            &items.folders_menu,
            &keep_menu,
            &keep_for_menu,
            &size_menu,
            &shortcut_menu,
            &items.open_screenshots,
            &PredefinedMenuItem::separator(app)?,
            &items.sound,
            &items.start_at_login,
            &items.check_updates,
            &language_menu,
            &PredefinedMenuItem::separator(app)?,
            &items.quit,
        ],
    )?;
    Ok((menu, items))
}

/// Clears and refills the two dynamic folder submenus from `state.extra_folders`.
fn refresh_folder_items(
    app: &AppHandle,
    _strings: &TrayStrings,
    items: &Items,
    state: &MenuState,
) -> tauri::Result<()> {
    clear_submenu(&items.folders_menu)?;
    clear_submenu(&items.stop_watching_menu)?;

    for (index, folder) in state.extra_folders.iter().enumerate() {
        let row = MenuItem::with_id(app, folder_open_id(index), folder, true, None::<&str>)?;
        items.folders_menu.append(&row)?;
    }
    if !state.extra_folders.is_empty() {
        items
            .folders_menu
            .append(&PredefinedMenuItem::separator(app)?)?;
    }
    items
        .folders_menu
        .set_text(i18n::tray_strings(state.lang).folders_menu)?;
    items.folders_menu.append(&items.add_folder)?;

    items
        .stop_watching_menu
        .set_enabled(!state.extra_folders.is_empty())?;
    if !state.extra_folders.is_empty() {
        items
            .folders_menu
            .append(&PredefinedMenuItem::separator(app)?)?;
        for (index, folder) in state.extra_folders.iter().enumerate() {
            let row = MenuItem::with_id(app, folder_stop_id(index), folder, true, None::<&str>)?;
            items.stop_watching_menu.append(&row)?;
        }
        items.folders_menu.append(&items.stop_watching_menu)?;
    }
    Ok(())
}

/// Removes every child of a submenu (dropping the old dynamic rows).
fn clear_submenu(menu: &Submenu<Wry>) -> tauri::Result<()> {
    while let Some(_item) = menu.remove_at(0)? {}
    Ok(())
}

/// Writes `state` into the existing native items (same language, so labels/texts are stable).
fn apply_state(
    app: &AppHandle,
    strings: &TrayStrings,
    items: &Items,
    state: &MenuState,
) -> tauri::Result<()> {
    items.toggle.set_text(state.toggle_label)?;
    items
        .toggle
        .set_accelerator(Some(state.toggle_hotkey.as_str()))?;
    items.capture.set_text(strings.capture)?;
    items
        .capture
        .set_accelerator(Some(state.capture_hotkey.as_str()))?;
    items
        .take_all_down
        .set_enabled(state.take_everything_down_enabled)?;

    items
        .handle_screenshots
        .set_text(strings.handle_screenshots)?;
    items
        .handle_screenshots
        .set_checked(state.handle_screenshots)?;
    items.hang_copied.set_text(strings.hang_copied_images)?;
    items.hang_copied.set_checked(state.hang_copied_images)?;
    items.handle_recordings.set_text(strings.handle_recordings)?;
    items
        .handle_recordings
        .set_checked(state.handle_recordings)?;
    items.reveal_on_new.set_text(strings.reveal_on_new)?;
    items.reveal_on_new.set_checked(state.reveal_on_new)?;
    items.top_edge_reveal.set_text(strings.top_edge_reveal)?;
    items.top_edge_reveal.set_checked(state.top_edge_reveal)?;
    items.over_full_screen.set_text(strings.show_over_full_screen)?;
    items
        .over_full_screen
        .set_checked(state.show_over_full_screen)?;

    items
        .add_folder
        .set_text(if state.add_folder_enabled {
            strings.add_folder
        } else {
            strings.add_folder_max
        })?;
    items.add_folder.set_enabled(state.add_folder_enabled)?;
    refresh_folder_items(app, strings, items, state)?;

    items.keep_fit.set_text(strings.keep_fit)?;
    items.keep_fit.set_checked(state.keep_on_line == KeepOnLine::Fit)?;
    items.keep_25.set_checked(state.keep_on_line == KeepOnLine::Limit(25))?;
    items.keep_50.set_checked(state.keep_on_line == KeepOnLine::Limit(50))?;
    items
        .keep_100
        .set_checked(state.keep_on_line == KeepOnLine::Limit(100))?;

    items.keep_forever.set_text(strings.keep_forever)?;
    items.keep_for_7.set_text(strings.keep_for_7)?;
    items.keep_for_30.set_text(strings.keep_for_30)?;
    items.keep_for_90.set_text(strings.keep_for_90)?;
    items
        .keep_forever
        .set_checked(state.keep_for_days == KeepForDays::Forever)?;
    items
        .keep_for_7
        .set_checked(state.keep_for_days == KeepForDays::Days(7))?;
    items
        .keep_for_30
        .set_checked(state.keep_for_days == KeepForDays::Days(30))?;
    items
        .keep_for_90
        .set_checked(state.keep_for_days == KeepForDays::Days(90))?;

    items.size_small.set_text(strings.size_small)?;
    items.size_medium.set_text(strings.size_medium)?;
    items.size_large.set_text(strings.size_large)?;
    items.size_small.set_checked(state.card_size == CardSize::Small)?;
    items
        .size_medium
        .set_checked(state.card_size == CardSize::Medium)?;
    items.size_large.set_checked(state.card_size == CardSize::Large)?;

    let failed_capture = state
        .hotkey_error
        .as_ref()
        .is_some_and(|failure| failure.target == Target::Capture);
    let failed_toggle = state
        .hotkey_error
        .as_ref()
        .is_some_and(|failure| failure.target == Target::Toggle);
    items.hotkey_capture.set_text(hotkey_row_label(
        strings.shortcut_capture,
        &state.capture_hotkey,
        failed_capture,
    ))?;
    items.hotkey_toggle.set_text(hotkey_row_label(
        strings.shortcut_toggle,
        &state.toggle_hotkey,
        failed_toggle,
    ))?;
    items.hotkey_status.set_text(hotkey_status_text(strings, state))?;

    items.open_screenshots.set_text(strings.open_screenshots)?;
    items.sound.set_text(strings.sound)?;
    items.sound.set_checked(state.sound)?;
    items.start_at_login.set_text(strings.start_at_login)?;
    items.start_at_login.set_checked(state.start_at_login)?;
    items.check_updates.set_text(strings.check_updates)?;
    items.language_system.set_text(strings.language_system)?;
    items.language_en.set_text(strings.language_en)?;
    items.language_zh.set_text(strings.language_zh_hans)?;
    items
        .language_system
        .set_checked(state.language.eq_ignore_ascii_case("system"))?;
    items
        .language_en
        .set_checked(state.language.eq_ignore_ascii_case("en"))?;
    items
        .language_zh
        .set_checked(state.language.eq_ignore_ascii_case("zh-Hans"))?;
    items.quit.set_text(strings.quit)?;
    Ok(())
}

/// Real login-item registration, falling back to the persisted setting when it cannot be read.
fn autostart_enabled(app: &AppHandle, settings: &Settings) -> bool {
    match crate::autostart::effective_enabled(app) {
        Some(enabled) => enabled,
        None => {
            log::debug!("tray: cannot read the autostart registration; using the setting");
            settings.start_at_login
        }
    }
}

/// The value a tray click must write: the negation of the *real* registration.
///
/// The check mark mirrors the registry, so a manual `Run` entry while `startAtLogin` is off shows
/// checked; the click then writes `false` and the shared settings path removes the entry. Using
/// `!settings.start_at_login` instead would write `true`, which leaves the existing entry untouched
/// and makes the first click look like a no-op.
fn start_at_login_target(effective: Option<bool>, setting: bool) -> bool {
    !effective.unwrap_or(setting)
}

/// Menu event entry point (main thread): dispatches the pure action on the right thread.
fn handle_menu(app: &AppHandle, id: &str) {
    let Some(action) = action_for(id) else {
        log::warn!("tray: unhandled menu item {id}");
        return;
    };
    log::info!("tray: menu action {} ({id})", action.as_str());
    match action {
        TrayAction::ToggleLine => reveal::toggle_from_pointer(app, RevealReason::Toggle),
        TrayAction::TakeScreenshot => {
            // Task 16: the same real capture path as `Ctrl+Alt+S`. Never run the request on the menu
            // handler thread (window creation dispatches to the event loop); the shared worker
            // dispatch is also what the hotkey uses (T16 review fix).
            log::info!("tray: capture requested (dispatching to the capture worker thread)");
            crate::capture::request_capture_in_background(app);
        }
        TrayAction::ChangeShortcut(target) => {
            if let Err(error) = crate::shortcut_window::open(app, target) {
                log::error!("tray: cannot open the change-shortcut window: {error}");
            }
        }
        // Everything below mutates settings / starts watchers / runs the shell: never on the main
        // thread (a watcher restart joins threads and the modal folder picker blocks until closed).
        TrayAction::TakeEverythingDown => spawn(app, "hangline-tray-take-down", |app| {
            actions::take_everything_down(&app);
        }),
        TrayAction::AddFolder => spawn(app, "hangline-tray-add-folder", |app| {
            actions::add_watched_folder(&app);
        }),
        TrayAction::OpenWatchedFolder(index) => spawn(app, "hangline-tray-open-folder", move |app| {
            actions::open_watched_folder(&app, index);
        }),
        TrayAction::StopWatching(index) => spawn(app, "hangline-tray-stop-watching", move |app| {
            actions::stop_watching_folder(&app, index);
        }),
        TrayAction::OpenScreenshotsFolder => spawn(app, "hangline-tray-open-shots", |app| {
            actions::open_screenshots_folder(&app);
        }),
        TrayAction::CheckForUpdates => spawn(app, "hangline-tray-updates", |app| {
            actions::check_for_updates(&app);
        }),
        TrayAction::Quit => spawn(app, "hangline-tray-quit", |app| {
            actions::quit(&app);
        }),
        action => spawn(app, "hangline-tray-setting", move |app| {
            apply_menu_action(&app, action);
        }),
    }
}

/// Applies a settings-changing tray action through the shared settings path (background thread).
fn apply_menu_action(app: &AppHandle, action: TrayAction) {
    let Some(snapshot) = app.try_state::<AppState>().map(|state| state.snapshot()) else {
        return;
    };
    let settings = snapshot.settings;
    let change = match action {
        TrayAction::ToggleWatchSystemScreenshots => Some((
            "watchSystemScreenshots",
            serde_json::json!(!settings.watch_system_screenshots),
        )),
        TrayAction::ToggleWatchRecordings => Some((
            "watchRecordings",
            serde_json::json!(!settings.watch_recordings),
        )),
        TrayAction::ToggleWatchClipboard => {
            Some(("watchClipboard", serde_json::json!(!settings.watch_clipboard)))
        }
        TrayAction::ToggleRevealOnNew => {
            Some(("revealOnNew", serde_json::json!(!settings.reveal_on_new)))
        }
        TrayAction::ToggleTopEdgeReveal => Some((
            "topEdgeReveal",
            serde_json::json!(!settings.top_edge_reveal),
        )),
        TrayAction::ToggleShowOverFullScreen => Some((
            "showOverFullScreen",
            serde_json::json!(!settings.show_over_full_screen),
        )),
        TrayAction::ToggleSound => Some(("sound", serde_json::json!(!settings.sound))),
        TrayAction::ToggleStartAtLogin => {
            let target = start_at_login_target(
                crate::autostart::effective_enabled(app),
                settings.start_at_login,
            );
            Some(("startAtLogin", serde_json::json!(target)))
        }
        TrayAction::SetKeepOnLine(keep) => Some(("keepOnLine", serde_json::json!(keep))),
        TrayAction::SetKeepForDays(policy) => Some(("keepForDays", serde_json::json!(policy))),
        TrayAction::SetCardSize(size) => Some(("cardSize", serde_json::json!(size))),
        TrayAction::SetLanguage(None) => Some(("language", serde_json::json!("system"))),
        TrayAction::SetLanguage(Some(lang)) => Some(("language", serde_json::json!(lang.tag()))),
        other => {
            log::warn!("tray: {} has no settings mapping", other.as_str());
            None
        }
    };
    let Some((key, value)) = change else {
        return;
    };
    match actions::apply(app, key, value.clone()) {
        Ok(_) => log::info!("tray: {key} set to {value}"),
        Err(error) => log::warn!("tray: cannot set {key} to {value}: {error}"),
    }
}

/// Runs `task` on a named background thread (menu actions must not block the main event loop).
fn spawn(app: &AppHandle, name: &str, task: impl FnOnce(AppHandle) + Send + 'static) {
    let app = app.clone();
    let builder = std::thread::Builder::new().name(name.to_string());
    match builder.spawn(move || task(app)) {
        Ok(handle) => {
            // The work is short-lived; detach it explicitly so a dropped handle is intentional.
            drop(handle);
        }
        Err(error) => log::error!("tray: cannot spawn the {name} thread: {error}"),
    }
}

/// Tray icon click: left click toggles the line (design spec §3.5 "双击 = 显示线" is folded into
/// the single click because the left-click menu is disabled; Task 3 decision kept).
fn handle_icon_event(app: &AppHandle, event: &TrayIconEvent) {
    if let TrayIconEvent::Click {
        button: MouseButton::Left,
        button_state: MouseButtonState::Up,
        ..
    } = event
    {
        log::info!("tray: left click toggle line");
        reveal::toggle_from_pointer(app, RevealReason::Toggle);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::settings::{KeepForDays, Theme};

    /// Plan Step 1 from the task brief: the menu model follows the panel visibility and settings.
    #[test]
    fn menu_labels_follow_state() {
        let settings = Settings::default();
        let visible = menu_state(&settings, true, 3, None, false, Lang::En);
        assert_eq!(visible.toggle_label, "Hide line");
        assert!(visible.handle_screenshots);
        assert_eq!(visible.keep_on_line, KeepOnLine::Fit);
        assert_eq!(visible.keep_for_days, KeepForDays::Forever);

        let hidden = menu_state(&settings, false, 0, None, false, Lang::En);
        assert_eq!(hidden.toggle_label, "Show line");
        assert!(!hidden.take_everything_down_enabled);
        assert_eq!(
            hidden.lang,
            Lang::En,
            "English is the fallback language for this state"
        );

        // The zh-Hans table drives the same dynamic fields.
        let chinese = menu_state(&settings, true, 2, None, false, Lang::ZhHans);
        assert_eq!(chinese.toggle_label, "隐藏线");
        assert!(chinese.take_everything_down_enabled);
    }

    /// The pure state copies the settings into the checkable/enabled properties the refresh uses.
    #[test]
    fn menu_state_maps_every_settings_switch() {
        let mut settings = Settings::default();
        settings.watch_system_screenshots = false;
        settings.watch_recordings = false;
        settings.watch_clipboard = false;
        settings.reveal_on_new = false;
        settings.top_edge_reveal = true;
        settings.show_over_full_screen = true;
        settings.sound = false;
        settings.start_at_login = true;
        settings.keep_on_line = KeepOnLine::Limit(50);
        settings.keep_for_days = KeepForDays::Days(30);
        settings.card_size = CardSize::Large;
        settings.extra_folders = vec![r"C:\Shots".to_string()];
        let state = menu_state(&settings, false, 1, None, true, Lang::En);
        assert!(!state.handle_screenshots);
        assert!(!state.handle_recordings);
        assert!(!state.hang_copied_images);
        assert!(!state.reveal_on_new);
        assert!(state.top_edge_reveal);
        assert!(state.show_over_full_screen);
        assert!(!state.sound);
        assert!(state.start_at_login);
        assert_eq!(state.keep_on_line, KeepOnLine::Limit(50));
        assert_eq!(state.keep_for_days, KeepForDays::Days(30));
        assert_eq!(state.card_size, CardSize::Large);
        assert_eq!(state.extra_folders, vec![r"C:\Shots".to_string()]);
        assert!(state.add_folder_enabled);
        assert_eq!(state.item_count, 1);
    }

    /// The tray check mark and the click target both follow the real registration, not the
    /// persisted setting: a manual `Run` entry (setting off, entry present) is checked, and the
    /// first click writes `false` (remove it). When the registry cannot be read the setting is the
    /// fallback and the click still flips it.
    #[test]
    fn start_at_login_toggle_targets_the_real_registration() {
        assert!(
            !start_at_login_target(Some(true), false),
            "a manual registration must be removed by the first click"
        );
        assert!(
            start_at_login_target(Some(false), true),
            "clicking while Task Manager-disabled enables it again"
        );
        assert!(start_at_login_target(Some(false), false));
        assert!(
            start_at_login_target(None, false),
            "unreadable registry falls back to the persisted setting"
        );
        assert!(!start_at_login_target(None, true));
    }

    /// The three-folder cap disables "Add Folder…" and is reflected in the tooltip-independent
    /// model; empty submenus stay disabled.
    #[test]
    fn add_folder_is_disabled_at_the_three_folder_cap() {
        let mut settings = Settings::default();
        assert!(menu_state(&settings, false, 0, None, false, Lang::En).add_folder_enabled);
        settings.extra_folders = vec![
            r"C:\One".to_string(),
            r"C:\Two".to_string(),
            r"C:\Three".to_string(),
        ];
        assert!(!menu_state(&settings, false, 0, None, false, Lang::En).add_folder_enabled);
    }

    /// Every static menu id maps to exactly the intended action; dynamic folder rows parse their
    /// index and unknown ids are a no-op.
    #[test]
    fn menu_ids_map_to_actions() {
        assert_eq!(action_for(ID_TOGGLE), Some(TrayAction::ToggleLine));
        assert_eq!(
            action_for(ID_TAKE_ALL_DOWN),
            Some(TrayAction::TakeEverythingDown)
        );
        assert_eq!(action_for(ID_CAPTURE), Some(TrayAction::TakeScreenshot));
        assert_eq!(
            action_for(ID_HANDLE_SCREENSHOTS),
            Some(TrayAction::ToggleWatchSystemScreenshots)
        );
        assert_eq!(
            action_for(ID_HANG_COPIED),
            Some(TrayAction::ToggleWatchClipboard)
        );
        assert_eq!(
            action_for(ID_HANDLE_RECORDINGS),
            Some(TrayAction::ToggleWatchRecordings)
        );
        assert_eq!(
            action_for(ID_REVEAL_ON_NEW),
            Some(TrayAction::ToggleRevealOnNew)
        );
        assert_eq!(
            action_for(ID_TOP_EDGE_REVEAL),
            Some(TrayAction::ToggleTopEdgeReveal)
        );
        assert_eq!(
            action_for(ID_OVER_FULL_SCREEN),
            Some(TrayAction::ToggleShowOverFullScreen)
        );
        assert_eq!(action_for(ID_ADD_FOLDER), Some(TrayAction::AddFolder));
        assert_eq!(
            action_for(ID_KEEP_FIT),
            Some(TrayAction::SetKeepOnLine(KeepOnLine::Fit))
        );
        assert_eq!(
            action_for(ID_KEEP_25),
            Some(TrayAction::SetKeepOnLine(KeepOnLine::Limit(25)))
        );
        assert_eq!(
            action_for(ID_KEEP_50),
            Some(TrayAction::SetKeepOnLine(KeepOnLine::Limit(50)))
        );
        assert_eq!(
            action_for(ID_KEEP_100),
            Some(TrayAction::SetKeepOnLine(KeepOnLine::Limit(100)))
        );
        assert_eq!(
            action_for(ID_KEEP_FOREVER),
            Some(TrayAction::SetKeepForDays(KeepForDays::Forever))
        );
        assert_eq!(
            action_for(ID_KEEP_FOR_7),
            Some(TrayAction::SetKeepForDays(KeepForDays::Days(7)))
        );
        assert_eq!(
            action_for(ID_KEEP_FOR_30),
            Some(TrayAction::SetKeepForDays(KeepForDays::Days(30)))
        );
        assert_eq!(
            action_for(ID_KEEP_FOR_90),
            Some(TrayAction::SetKeepForDays(KeepForDays::Days(90)))
        );
        assert_eq!(
            action_for(ID_SIZE_SMALL),
            Some(TrayAction::SetCardSize(CardSize::Small))
        );
        assert_eq!(
            action_for(ID_SIZE_MEDIUM),
            Some(TrayAction::SetCardSize(CardSize::Medium))
        );
        assert_eq!(
            action_for(ID_SIZE_LARGE),
            Some(TrayAction::SetCardSize(CardSize::Large))
        );
        assert_eq!(
            action_for(ID_HOTKEY_CAPTURE),
            Some(TrayAction::ChangeShortcut(Target::Capture))
        );
        assert_eq!(
            action_for(ID_HOTKEY_TOGGLE),
            Some(TrayAction::ChangeShortcut(Target::Toggle))
        );
        assert_eq!(action_for(ID_HOTKEY_STATUS), None);
        assert_eq!(
            action_for(ID_OPEN_SCREENSHOTS),
            Some(TrayAction::OpenScreenshotsFolder)
        );
        assert_eq!(action_for(ID_SOUND), Some(TrayAction::ToggleSound));
        assert_eq!(
            action_for(ID_START_AT_LOGIN),
            Some(TrayAction::ToggleStartAtLogin)
        );
        assert_eq!(
            action_for(ID_CHECK_UPDATES),
            Some(TrayAction::CheckForUpdates)
        );
        assert_eq!(
            action_for(ID_LANGUAGE_SYSTEM),
            Some(TrayAction::SetLanguage(None))
        );
        assert_eq!(
            action_for(ID_LANGUAGE_EN),
            Some(TrayAction::SetLanguage(Some(Lang::En)))
        );
        assert_eq!(
            action_for(ID_LANGUAGE_ZH),
            Some(TrayAction::SetLanguage(Some(Lang::ZhHans)))
        );
        assert_eq!(action_for(ID_QUIT), Some(TrayAction::Quit));
        assert_eq!(action_for("tray.unknown"), None);
        assert_eq!(action_for("tray.folder.open.not-a-number"), None);
        assert_eq!(action_for(""), None);
    }

    /// Dynamic watched-folder rows round-trip through their id.
    #[test]
    fn dynamic_folder_ids_round_trip() {
        for index in [0usize, 1, 2, 42] {
            assert_eq!(
                action_for(&folder_open_id(index)),
                Some(TrayAction::OpenWatchedFolder(index))
            );
            assert_eq!(
                action_for(&folder_stop_id(index)),
                Some(TrayAction::StopWatching(index))
            );
        }
        assert_eq!(folder_open_id(0), "tray.folder.open.0");
        assert_eq!(folder_stop_id(2), "tray.folder.stop.2");
        assert_ne!(folder_open_id(1), folder_stop_id(1));
    }

    /// The tooltip carries the item count (spec §3.11) and appends the shortcut failure suffix.
    #[test]
    fn tooltip_reports_item_count_and_hotkey_failure() {
        let strings = i18n::tray_strings(Lang::En);
        let mut state = menu_state(&Settings::default(), false, 0, None, false, Lang::En);
        assert_eq!(tooltip(strings, &state), "Hangline — 0 card(s) on the line");
        state.item_count = 4;
        assert_eq!(tooltip(strings, &state), "Hangline — 4 card(s) on the line");
        state.hotkey_error = Some(HotkeyFailure {
            target: Target::Capture,
            accelerator: "Ctrl+Alt+S".to_string(),
            message: "HotKey already registered".to_string(),
        });
        assert_eq!(
            tooltip(strings, &state),
            "Hangline — 4 card(s) on the line — shortcut unavailable"
        );
        let zh = i18n::tray_strings(Lang::ZhHans);
        let zh_state = menu_state(&Settings::default(), false, 2, None, false, Lang::ZhHans);
        assert_eq!(tooltip(zh, &zh_state), "Hangline — 线上 2 张");
    }

    /// A hotkey failure marks the matching shortcut row and the disabled status row; the other row
    /// stays clean.
    #[test]
    fn hotkey_failure_marks_the_matching_row_and_status() {
        let strings = i18n::tray_strings(Lang::En);
        let failure = HotkeyFailure {
            target: Target::Toggle,
            accelerator: "Ctrl+Alt+T".to_string(),
            message: "owned by another app".to_string(),
        };
        let state = menu_state(
            &Settings::default(),
            false,
            0,
            Some(failure.clone()),
            false,
            Lang::En,
        );
        assert_eq!(
            hotkey_row_label(strings.shortcut_capture, "Ctrl+Alt+S", false),
            "Capture: Ctrl+Alt+S"
        );
        assert_eq!(
            hotkey_row_label(strings.shortcut_toggle, "Ctrl+Alt+T", true),
            "Show/hide line: Ctrl+Alt+T ⚠"
        );
        let status = hotkey_status_text(strings, &state);
        assert!(status.contains("Shortcut unavailable"), "got {status:?}");
        assert!(status.contains("Show/hide line"), "got {status:?}");
        assert!(status.contains("owned by another app"), "got {status:?}");

        let ok = menu_state(&Settings::default(), false, 0, None, false, Lang::En);
        assert_eq!(hotkey_status_text(strings, &ok), "Hotkeys are active");
        assert_eq!(
            state.hotkey_error.as_ref().map(|failure| failure.target),
            Some(Target::Toggle)
        );
    }

    /// The zh-Hans tray model uses the translated static labels for the disabled status row.
    #[test]
    fn chinese_status_row_is_translated() {
        let strings = i18n::tray_strings(Lang::ZhHans);
        let ok = menu_state(&Settings::default(), false, 0, None, false, Lang::ZhHans);
        assert_eq!(hotkey_status_text(strings, &ok), "快捷键已启用");
        assert_eq!(strings.add_folder_max, "添加文件夹…（已达上限 3 个）");
        // Language radio checks follow the persisted setting + effective language.
        let mut settings = Settings::default();
        settings.language = "zh-Hans".to_string();
        let zh_state = menu_state(&settings, false, 0, None, false, Lang::ZhHans);
        assert_eq!(zh_state.lang, Lang::ZhHans);
        assert_eq!(zh_state.language, "zh-Hans");
        assert_eq!(
            settings.theme,
            Theme::System,
            "theme stays a frontend-only setting"
        );
    }
}
