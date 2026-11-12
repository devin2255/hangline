//! Rust-side UI strings for the tray menu (design spec §3.5/§3.10).
//!
//! The tray/menu tree is built in Rust, so it cannot read the frontend's `src/i18n/*.json`
//! tables. This module owns the Rust half of the i18n contract:
//!
//! - [`resolve_language`] maps the persisted `language` setting (`"system"` or a BCP 47 tag)
//!   to a supported [`Lang`], falling back to English for anything unknown (spec §3.10).
//! - [`tray_strings`] returns the frozen [`TrayStrings`] table for a language. The table is a
//!   struct (not a JSON map) so a missing translation is a compile error instead of a runtime
//!   fallback; [`TrayStrings::all`] exposes the same rows as key/value pairs for the parity test.
//!
//! Every user-visible tray/ native-window label lives here. Log lines stay English on purpose
//! (they are diagnostic text, not UI), matching the rest of the Rust code.

use tauri::Manager;
use windows::core::PWSTR;
use windows::Win32::Globalization::{
    GetUserDefaultLocaleName, GetUserDefaultUILanguage, GetUserPreferredUILanguages,
    MUI_LANGUAGE_NAME,
};

/// A language the Rust UI layer can render.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    /// English (the fallback for every unknown/unsupported tag).
    En,
    /// Simplified Chinese (`zh-Hans`).
    ZhHans,
}

impl Lang {
    /// The BCP 47 tag persisted in `settings.language` for this language.
    pub fn tag(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::ZhHans => "zh-Hans",
        }
    }
}

/// Resolves the `language` setting to the language Rust must render.
///
/// `"system"` follows the OS UI/display language tag (`system_tag`, e.g. `"zh-CN"`); any `zh-*`
/// locale maps to Simplified Chinese (V1 ships `zh-Hans` only and prefers Chinese over the English
/// fallback for Chinese Windows installations), and every other/unknown tag falls back to English
/// (spec §3.10: missing translations fall back to `en`).
pub fn resolve_language(setting: &str, system_tag: &str) -> Lang {
    let tag = if setting.eq_ignore_ascii_case("system") {
        system_tag
    } else {
        setting
    };
    let tag = tag.trim().to_ascii_lowercase();
    if tag == "zh" || tag.starts_with("zh-") || tag.starts_with("zh_") {
        Lang::ZhHans
    } else {
        Lang::En
    }
}

/// The resolved tag (`"en"` or `"zh-Hans"`) for a `language` setting against a system tag.
///
/// This is the value broadcast to the frontend as `PublicState::resolved_language`: both the Rust
/// tray and the webview must derive the active language from the *same* resolution, otherwise a
/// mixed-locale Windows install (display language different from the regional format) could show a
/// Chinese tray menu next to an English panel under the default `"system"` setting.
pub fn resolved_tag_for(setting: &str, system_tag: &str) -> &'static str {
    resolve_language(setting, system_tag).tag()
}

/// The resolved tag for a setting against the current Windows UI language.
pub fn resolved_tag(setting: &str) -> &'static str {
    resolved_tag_for(setting, system_language())
}

/// Display/UI language tag of the current Windows user (e.g. `"zh-CN"`).
///
/// The tray/panel language follows the **Windows display language** (the "Windows display language"
/// setting, exposed by `GetUserPreferredUILanguages`), not the regional-format locale
/// (`GetUserDefaultLocaleName`) and not the webview's `navigator.language`: on a mixed-locale
/// install those can disagree, which used to make the tray and the panel pick different languages
/// for `language: "system"`. The fallback chain is
///
/// 1. first preferred UI language name (`GetUserPreferredUILanguages(MUI_LANGUAGE_NAME)`),
/// 2. `GetUserDefaultUILanguage` mapped to `"zh-Hans"`/`"en"`,
/// 3. the regional locale (`GetUserDefaultLocaleName`),
/// 4. `"en"`.
///
/// The value is cached for the process lifetime: the tray re-reads it on every state broadcast and
/// a Windows display-language change takes effect on the next launch.
pub fn system_language() -> &'static str {
    static SYSTEM_LANGUAGE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    SYSTEM_LANGUAGE.get_or_init(read_system_language)
}

/// Reads the OS display language (the uncached half of [`system_language`]).
fn read_system_language() -> String {
    let preferred = preferred_ui_language();
    let ui = ui_language_tag();
    let locale = default_locale_name();
    pick_system_language(preferred.as_deref(), ui.as_deref(), locale.as_deref())
}

/// Pure fallback chain for [`read_system_language`] (unit-tested for the mixed-locale cases).
///
/// Empty/whitespace tags count as missing; the result is always a non-empty tag.
pub fn pick_system_language(
    preferred_ui: Option<&str>,
    default_ui: Option<&str>,
    locale: Option<&str>,
) -> String {
    for candidate in [preferred_ui, default_ui, locale] {
        if let Some(tag) = candidate.map(str::trim).filter(|tag| !tag.is_empty()) {
            return tag.to_string();
        }
    }
    "en".to_string()
}

/// First preferred display-language name (`MUI_LANGUAGE_NAME`), e.g. `"zh-CN"`.
fn preferred_ui_language() -> Option<String> {
    let mut count: u32 = 0;
    let mut size: u32 = 0;
    // SAFETY: the size query passes a null buffer with a valid size pointer; the win32 wrapper
    // passes the null PWSTR through unchanged and returns an error result on failure.
    unsafe { GetUserPreferredUILanguages(MUI_LANGUAGE_NAME, &mut count, None, &mut size) }.ok()?;
    if count == 0 || size == 0 {
        return None;
    }
    let mut buffer = vec![0u16; size as usize];
    // SAFETY: `buffer` is writable for `size` u16 units and `size` is the API-reported length.
    unsafe {
        GetUserPreferredUILanguages(
            MUI_LANGUAGE_NAME,
            &mut count,
            Some(PWSTR(buffer.as_mut_ptr())),
            &mut size,
        )
    }
    .ok()?;
    // The buffer is a double-NUL-terminated list of names; the first one is the display language.
    let first = buffer.split(|unit| *unit == 0).next()?;
    let tag = String::from_utf16_lossy(first);
    (!tag.trim().is_empty()).then(|| tag.trim().to_string())
}

/// Maps the `GetUserDefaultUILanguage` LANGID to one of the two supported language families.
///
/// Only the primary language id is needed for the V1 language set: `LANG_CHINESE` (0x04) selects
/// Simplified Chinese (traditional Chinese is not shipped in V1 and falls back like any other
/// unsupported UI language to English), everything else selects English.
fn ui_language_tag() -> Option<String> {
    // SAFETY: GetUserDefaultUILanguage reads a user preference and takes no arguments.
    let langid = unsafe { GetUserDefaultUILanguage() };
    if langid == 0 {
        return None;
    }
    let primary = langid & 0x3ff;
    Some(if primary == 0x04 { "zh-Hans" } else { "en" }.to_string())
}

/// Regional-format locale (`GetUserDefaultLocaleName`), used only as the last fallback.
fn default_locale_name() -> Option<String> {
    let mut buffer = [0u16; 85]; // LOCALE_NAME_MAX_LENGTH on current Windows.
    // SAFETY: the buffer is writable for its whole length; the API writes a NUL-terminated
    // locale name and returns the number of u16 units written (including the terminator).
    let written = unsafe { GetUserDefaultLocaleName(&mut buffer) };
    if written <= 1 {
        return None;
    }
    let len = (written as usize - 1).min(buffer.len());
    let tag = String::from_utf16_lossy(&buffer[..len]);
    (!tag.trim().is_empty()).then(|| tag.trim().to_string())
}

/// The language the Rust UI (tray, native picker, shortcut window title) must render right now.
///
/// Resolves the persisted `language` setting against the Windows display language; an unmanaged
/// `AppState` (early startup, tests) behaves like the spec default `"system"`.
pub fn app_language(app: &tauri::AppHandle) -> Lang {
    let setting = app
        .try_state::<crate::app_state::AppState>()
        .map(|state| state.snapshot().settings.language)
        .unwrap_or_else(|| "system".to_string());
    resolve_language(&setting, system_language())
}

/// Locale used for the `language: "system"` setting (test seam).
///
/// The production code calls [`app_language`]; tests can substitute a deterministic locale.
#[cfg(test)]
pub fn effective_language(setting: &str, system_tag: &str) -> Lang {
    resolve_language(setting, system_tag)
}

/// Static labels for the complete tray tree, shortcut window and folder picker (spec §3.5).
///
/// All fields are `&'static str`: the tables are compile-time constants and [`tray_strings`]
/// hands out a reference, so menu refresh never allocates per label.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrayStrings {
    /// Tray icon tooltip when items are on the line (`{}` = item count).
    pub tooltip_items: &'static str,
    /// Tray tooltip suffix when a global shortcut could not be registered.
    pub tooltip_hotkey_unavailable: &'static str,
    /// Toggle item while the panel is hidden.
    pub show_line: &'static str,
    /// Toggle item while the panel is visible.
    pub hide_line: &'static str,
    /// "Take everything down" action.
    pub take_everything_down: &'static str,
    /// W1.7 confirmation title for "Take everything down".
    pub take_all_down_confirm_title: &'static str,
    /// W1.7 confirmation body (`{}` = item count; explains the Recycle Bin/kept files).
    pub take_all_down_confirm_body: &'static str,
    /// "Take screenshot…" action (wired to the capture request until Task 16).
    pub capture: &'static str,
    /// W1.5 switch: copy a confirmed screenshot to the clipboard after saving.
    pub capture_copy_on_finish: &'static str,
    /// "Search…" action that opens the on-demand search window (Wave 2 W2.2/W2.3).
    pub search: &'static str,
    /// System-screenshot directory watcher switch.
    pub handle_screenshots: &'static str,
    /// Recordings directory watcher switch.
    pub handle_recordings: &'static str,
    /// Clipboard image listener switch.
    pub hang_copied_images: &'static str,
    /// Reveal-behaviour submenu title.
    pub reveal_menu: &'static str,
    /// Reveal on a new screenshot.
    pub reveal_on_new: &'static str,
    /// Reveal when the pointer rests on the top edge.
    pub top_edge_reveal: &'static str,
    /// Keep the line over full-screen apps.
    pub show_over_full_screen: &'static str,
    /// Watched-folders submenu title.
    pub folders_menu: &'static str,
    /// Open the system folder picker.
    pub add_folder: &'static str,
    /// Add-folder item once the three-folder limit is reached.
    pub add_folder_max: &'static str,
    /// Submenu that removes watched folders.
    pub stop_watching_menu: &'static str,
    /// Keep-count submenu title.
    pub keep_menu: &'static str,
    /// Keep as many cards as fit.
    pub keep_fit: &'static str,
    /// Age-retention submenu title.
    pub keep_for_menu: &'static str,
    /// Keep every card forever (age retention off, the default).
    pub keep_forever: &'static str,
    /// Keep the last 7 days (expired Hangline-owned screenshots are recycled).
    pub keep_for_7: &'static str,
    /// Keep the last 30 days.
    pub keep_for_30: &'static str,
    /// Keep the last 90 days.
    pub keep_for_90: &'static str,
    /// W1.7 disabled explanation row inside the "Keep on line" submenu.
    pub keep_info: &'static str,
    /// W1.7 disabled explanation row inside the "Keep for" submenu.
    pub keep_for_info: &'static str,
    /// Card-size submenu title.
    pub size_menu: &'static str,
    /// Small cards.
    pub size_small: &'static str,
    /// Medium cards.
    pub size_medium: &'static str,
    /// Large cards.
    pub size_large: &'static str,
    /// v1.4 "Appearance" submenu title (rope style/colour).
    pub appearance_menu: &'static str,
    /// "Rope style" submenu title.
    pub rope_style_menu: &'static str,
    /// "Rope color" submenu title.
    pub rope_color_menu: &'static str,
    /// Rope style: smooth silk ribbon (default).
    pub rope_style_silk: &'static str,
    /// Rope style: woven linen cord.
    pub rope_style_linen: &'static str,
    /// Rope style: brass chain.
    pub rope_style_brass: &'static str,
    /// Rope style: aurora gradient ribbon.
    pub rope_style_aurora: &'static str,
    /// Rope colour: ink black (default).
    pub rope_color_ink: &'static str,
    /// Rope colour: ivory white.
    pub rope_color_ivory: &'static str,
    /// Rope colour: champagne gold.
    pub rope_color_champagne: &'static str,
    /// Rope colour: fog grey.
    pub rope_color_fog: &'static str,
    /// Rope colour: sage green.
    pub rope_color_sage: &'static str,
    /// Rope colour: haze blue.
    pub rope_color_haze: &'static str,
    /// Rope colour: terracotta.
    pub rope_color_terracotta: &'static str,
    /// Rope colour: violet.
    pub rope_color_violet: &'static str,
    /// Shortcut submenu title.
    pub shortcut_menu: &'static str,
    /// Capture shortcut row (followed by the accelerator).
    pub shortcut_capture: &'static str,
    /// Show/hide shortcut row (followed by the accelerator).
    pub shortcut_toggle: &'static str,
    /// W1.8 "edit the newest card" shortcut row (followed by the accelerator).
    pub shortcut_edit_latest: &'static str,
    /// Wave 2 "search" shortcut row (followed by the accelerator).
    pub shortcut_search: &'static str,
    /// Opens the change-shortcut window.
    pub shortcut_change: &'static str,
    /// Disabled status row when every shortcut is registered.
    pub hotkeys_ok: &'static str,
    /// Disabled status row when a shortcut could not be registered.
    pub hotkey_unavailable: &'static str,
    /// Opens Hangline's own screenshot directory.
    pub open_screenshots: &'static str,
    /// Copy/discard sound switch.
    pub sound: &'static str,
    /// Start-at-login switch.
    pub start_at_login: &'static str,
    /// Manual update check.
    pub check_updates: &'static str,
    /// Language submenu title.
    pub language_menu: &'static str,
    /// Follow the Windows UI language.
    pub language_system: &'static str,
    /// English label (native name, identical in both tables).
    pub language_en: &'static str,
    /// Simplified Chinese label (native name, identical in both tables).
    pub language_zh_hans: &'static str,
    /// Quit action.
    pub quit: &'static str,
    /// Title of the system folder picker.
    pub picker_title: &'static str,
    /// Title of the minimal change-shortcut window.
    pub shortcut_window_title: &'static str,
    /// Title of the on-demand search window (Wave 2 W2.2).
    pub search_window_title: &'static str,
    /// W1.7 startup warning title (a global shortcut could not be registered).
    pub hotkey_error_title: &'static str,
    /// W1.7 startup warning intro line.
    pub hotkey_error_intro: &'static str,
    /// W1.7 one failure row (`{}` = label, `{}` = accelerator, `{}` = reason).
    pub hotkey_error_detail: &'static str,
    /// W1.7 startup warning hint (where the shortcut can be changed).
    pub hotkey_error_hint: &'static str,
}

/// English strings (the fallback language).
pub const EN: TrayStrings = TrayStrings {
    tooltip_items: "Hangline — {} card(s) on the line",
    tooltip_hotkey_unavailable: " — shortcut unavailable",
    show_line: "Show line",
    hide_line: "Hide line",
    take_everything_down: "Take everything down",
    take_all_down_confirm_title: "Take everything down?",
    take_all_down_confirm_body: "Take all {} card(s) off the line?\n\nHangline's own screenshots go to the Recycle Bin (you can restore them); files from watched folders and system folders are not deleted.",
    capture: "Take screenshot…",
    capture_copy_on_finish: "Copy after screenshot",
    search: "Search…",
    handle_screenshots: "Handle system screenshots",
    handle_recordings: "Handle recordings",
    hang_copied_images: "Hang copied images",
    reveal_menu: "Bring the line down",
    reveal_on_new: "On new screenshots",
    top_edge_reveal: "When the pointer rests on the top edge",
    show_over_full_screen: "Over full-screen apps",
    folders_menu: "Also watch folders",
    add_folder: "Add Folder…",
    add_folder_max: "Add Folder… (3/3)",
    stop_watching_menu: "Stop watching",
    keep_menu: "Keep on line",
    keep_fit: "As many as fit",
    keep_for_menu: "Keep for",
    keep_forever: "Keep forever",
    keep_for_7: "Keep last 7 days",
    keep_for_30: "Keep last 30 days",
    keep_for_90: "Keep last 90 days",
    keep_info: "Cards only leave the line; files are not deleted",
    keep_for_info: "Expired Hangline screenshots go to the Recycle Bin (undoable)",
    size_menu: "Size",
    size_small: "Small",
    size_medium: "Medium",
    size_large: "Large",
    appearance_menu: "Appearance",
    rope_style_menu: "Rope style",
    rope_color_menu: "Rope color",
    rope_style_silk: "Silk",
    rope_style_linen: "Linen",
    rope_style_brass: "Brass",
    rope_style_aurora: "Aurora",
    rope_color_ink: "Ink",
    rope_color_ivory: "Ivory",
    rope_color_champagne: "Champagne",
    rope_color_fog: "Fog",
    rope_color_sage: "Sage",
    rope_color_haze: "Haze",
    rope_color_terracotta: "Terracotta",
    rope_color_violet: "Violet",
    shortcut_menu: "Shortcut",
    shortcut_capture: "Capture",
    shortcut_toggle: "Show/hide line",
    shortcut_edit_latest: "Edit latest",
    shortcut_search: "Search",
    shortcut_change: "Change Shortcut…",
    hotkeys_ok: "Hotkeys are active",
    hotkey_unavailable: "Shortcut unavailable",
    open_screenshots: "Open screenshots folder",
    sound: "Sounds",
    start_at_login: "Open at login",
    check_updates: "Check for updates…",
    language_menu: "Language",
    language_system: "Follow system",
    language_en: "English",
    language_zh_hans: "简体中文",
    quit: "Quit Hangline",
    picker_title: "Choose a folder to watch",
    shortcut_window_title: "Hangline — Change shortcut",
    search_window_title: "Hangline — Search",
    hotkey_error_title: "Hangline — Shortcut unavailable",
    hotkey_error_intro: "Hangline could not register these global shortcuts:",
    hotkey_error_detail: "• {} ({}): {}",
    hotkey_error_hint: "Change a shortcut in the tray menu → Shortcut.",
};

/// Simplified Chinese strings (`zh-Hans`).
pub const ZH_HANS: TrayStrings = TrayStrings {
    tooltip_items: "Hangline — 线上 {} 张",
    tooltip_hotkey_unavailable: " — 快捷键不可用",
    show_line: "显示线",
    hide_line: "隐藏线",
    take_everything_down: "全部取下线",
    take_all_down_confirm_title: "全部取下线？",
    take_all_down_confirm_body: "将把 {} 张卡片全部从线上取下。\n\nHangline 自有目录中的截图会进入回收站（可撤销）；监视文件夹与系统文件不会被删除。",
    capture: "截图…",
    capture_copy_on_finish: "截图后自动复制",
    search: "搜索…",
    handle_screenshots: "处理系统截图",
    handle_recordings: "挂起录屏",
    hang_copied_images: "挂起复制的图片",
    reveal_menu: "线出现方式",
    reveal_on_new: "新截图时",
    top_edge_reveal: "指针停在顶边时",
    show_over_full_screen: "全屏应用上",
    folders_menu: "也监听文件夹",
    add_folder: "添加文件夹…",
    add_folder_max: "添加文件夹…（已达上限 3 个）",
    stop_watching_menu: "停止监听",
    keep_menu: "保留数量",
    keep_fit: "放得下多少",
    keep_for_menu: "保留策略",
    keep_forever: "永久保留",
    keep_for_7: "保留最近 7 天",
    keep_for_30: "保留最近 30 天",
    keep_for_90: "保留最近 90 天",
    keep_info: "卡片只从线上移除，文件不会被删除",
    keep_for_info: "到期的自有截图会进入回收站（可撤销）",
    size_menu: "尺寸",
    size_small: "小",
    size_medium: "中",
    size_large: "大",
    appearance_menu: "外观",
    rope_style_menu: "绳子样式",
    rope_color_menu: "绳子颜色",
    rope_style_silk: "真丝",
    rope_style_linen: "亚麻",
    rope_style_brass: "黄铜链",
    rope_style_aurora: "暮光线",
    rope_color_ink: "墨",
    rope_color_ivory: "象牙",
    rope_color_champagne: "香槟金",
    rope_color_fog: "雾灰",
    rope_color_sage: "鼠尾草",
    rope_color_haze: "雾霾蓝",
    rope_color_terracotta: "赤陶",
    rope_color_violet: "烟紫",
    shortcut_menu: "快捷键",
    shortcut_capture: "截图",
    shortcut_toggle: "显示/隐藏线",
    shortcut_edit_latest: "编辑最新一张",
    shortcut_search: "搜索",
    shortcut_change: "修改快捷键…",
    hotkeys_ok: "快捷键已启用",
    hotkey_unavailable: "快捷键不可用",
    open_screenshots: "打开截图文件夹",
    sound: "声音",
    start_at_login: "开机启动",
    check_updates: "检查更新…",
    language_menu: "语言",
    language_system: "跟随系统",
    language_en: "English",
    language_zh_hans: "简体中文",
    quit: "退出 Hangline",
    picker_title: "选择要监听的文件夹",
    shortcut_window_title: "Hangline — 修改快捷键",
    search_window_title: "Hangline — 搜索",
    hotkey_error_title: "Hangline — 快捷键不可用",
    hotkey_error_intro: "Hangline 无法注册以下全局快捷键：",
    hotkey_error_detail: "• {}（{}）：{}",
    hotkey_error_hint: "可在托盘菜单 → 快捷键 中修改。",
};

impl TrayStrings {
    /// The table for `lang` (never fails: every [`Lang`] has a full table).
    pub fn for_language(lang: Lang) -> &'static TrayStrings {
        match lang {
            Lang::En => &EN,
            Lang::ZhHans => &ZH_HANS,
        }
    }

    /// Every user-visible string as a `(key, value)` row (test-only parity check).
    ///
    /// The parity test asserts both languages expose the same keys and every value is non-empty;
    /// a new field added to the struct without being added here fails that test, which keeps the
    /// row list in sync with the table.
    #[cfg(test)]
    pub fn all(&self) -> Vec<(&'static str, &'static str)> {
        vec![
            ("tooltip_items", self.tooltip_items),
            ("tooltip_hotkey_unavailable", self.tooltip_hotkey_unavailable),
            ("show_line", self.show_line),
            ("hide_line", self.hide_line),
            ("take_everything_down", self.take_everything_down),
            ("take_all_down_confirm_title", self.take_all_down_confirm_title),
            ("take_all_down_confirm_body", self.take_all_down_confirm_body),
            ("capture", self.capture),
            ("capture_copy_on_finish", self.capture_copy_on_finish),
            ("search", self.search),
            ("handle_screenshots", self.handle_screenshots),
            ("handle_recordings", self.handle_recordings),
            ("hang_copied_images", self.hang_copied_images),
            ("reveal_menu", self.reveal_menu),
            ("reveal_on_new", self.reveal_on_new),
            ("top_edge_reveal", self.top_edge_reveal),
            ("show_over_full_screen", self.show_over_full_screen),
            ("folders_menu", self.folders_menu),
            ("add_folder", self.add_folder),
            ("add_folder_max", self.add_folder_max),
            ("stop_watching_menu", self.stop_watching_menu),
            ("keep_menu", self.keep_menu),
            ("keep_fit", self.keep_fit),
            ("keep_for_menu", self.keep_for_menu),
            ("keep_forever", self.keep_forever),
            ("keep_for_7", self.keep_for_7),
            ("keep_for_30", self.keep_for_30),
            ("keep_for_90", self.keep_for_90),
            ("keep_info", self.keep_info),
            ("keep_for_info", self.keep_for_info),
            ("size_menu", self.size_menu),
            ("size_small", self.size_small),
            ("size_medium", self.size_medium),
            ("size_large", self.size_large),
            ("appearance_menu", self.appearance_menu),
            ("rope_style_menu", self.rope_style_menu),
            ("rope_color_menu", self.rope_color_menu),
            ("rope_style_silk", self.rope_style_silk),
            ("rope_style_linen", self.rope_style_linen),
            ("rope_style_brass", self.rope_style_brass),
            ("rope_style_aurora", self.rope_style_aurora),
            ("rope_color_ink", self.rope_color_ink),
            ("rope_color_ivory", self.rope_color_ivory),
            ("rope_color_champagne", self.rope_color_champagne),
            ("rope_color_fog", self.rope_color_fog),
            ("rope_color_sage", self.rope_color_sage),
            ("rope_color_haze", self.rope_color_haze),
            ("rope_color_terracotta", self.rope_color_terracotta),
            ("rope_color_violet", self.rope_color_violet),
            ("shortcut_menu", self.shortcut_menu),
            ("shortcut_capture", self.shortcut_capture),
            ("shortcut_toggle", self.shortcut_toggle),
            ("shortcut_edit_latest", self.shortcut_edit_latest),
            ("shortcut_search", self.shortcut_search),
            ("shortcut_change", self.shortcut_change),
            ("hotkeys_ok", self.hotkeys_ok),
            ("hotkey_unavailable", self.hotkey_unavailable),
            ("open_screenshots", self.open_screenshots),
            ("sound", self.sound),
            ("start_at_login", self.start_at_login),
            ("check_updates", self.check_updates),
            ("language_menu", self.language_menu),
            ("language_system", self.language_system),
            ("language_en", self.language_en),
            ("language_zh_hans", self.language_zh_hans),
            ("quit", self.quit),
            ("picker_title", self.picker_title),
            ("shortcut_window_title", self.shortcut_window_title),
            ("search_window_title", self.search_window_title),
            ("hotkey_error_title", self.hotkey_error_title),
            ("hotkey_error_intro", self.hotkey_error_intro),
            ("hotkey_error_detail", self.hotkey_error_detail),
            ("hotkey_error_hint", self.hotkey_error_hint),
        ]
    }

    /// W1.7 "take everything down" confirmation body with the item count filled in.
    pub fn take_all_down_body(&self, count: usize) -> String {
        self.take_all_down_confirm_body
            .replacen("{}", &count.to_string(), 1)
    }

    /// W1.7 one startup-warning row: `• <label> (<accelerator>): <reason>`.
    pub fn hotkey_error_detail_line(
        &self,
        label: &str,
        accelerator: &str,
        reason: &str,
    ) -> String {
        self.hotkey_error_detail
            .replacen("{}", label, 1)
            .replacen("{}", accelerator, 1)
            .replacen("{}", reason, 1)
    }
}

/// Returns the frozen table for `lang`.
pub fn tray_strings(lang: Lang) -> &'static TrayStrings {
    TrayStrings::for_language(lang)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two V1 tables must expose exactly the same key set (design spec §3.10) and no empty
    /// value: a missing Chinese string would render as a blank menu row.
    #[test]
    fn en_and_zh_hans_tables_have_key_parity_and_no_blanks() {
        let en = EN.all();
        let zh = ZH_HANS.all();
        let en_keys: Vec<&str> = en.iter().map(|(key, _)| *key).collect();
        let zh_keys: Vec<&str> = zh.iter().map(|(key, _)| *key).collect();
        assert_eq!(en_keys, zh_keys, "en/zh-Hans key lists must match");
        for (key, value) in en.iter().chain(zh.iter()) {
            assert!(
                !value.trim().is_empty(),
                "{key} must not be empty in both languages"
            );
        }
        // Native language names stay untranslated in both tables.
        assert_eq!(EN.language_en, "English");
        assert_eq!(ZH_HANS.language_en, "English");
        assert_eq!(EN.language_zh_hans, "简体中文");
        assert_eq!(ZH_HANS.language_zh_hans, "简体中文");
        // The tray toggle labels are distinct in both languages (the label follows panel state).
        assert_ne!(EN.show_line, EN.hide_line);
        assert_ne!(ZH_HANS.show_line, ZH_HANS.hide_line);
        // Age retention (keepForDays) belongs to the same frozen tray table in both languages.
        assert_eq!(EN.keep_for_menu, "Keep for");
        assert_eq!(ZH_HANS.keep_for_menu, "保留策略");
        assert_eq!(EN.keep_forever, "Keep forever");
        assert_eq!(ZH_HANS.keep_forever, "永久保留");
    }

    #[test]
    fn language_resolution_follows_spec_fallback() {
        // `system` maps through the OS locale.
        assert_eq!(resolve_language("system", "en-US"), Lang::En);
        assert_eq!(resolve_language("system", "zh-CN"), Lang::ZhHans);
        assert_eq!(resolve_language("system", "zh-Hans-CN"), Lang::ZhHans);
        assert_eq!(resolve_language("system", "zh-TW"), Lang::ZhHans);
        // Explicit tags win over the OS locale.
        assert_eq!(resolve_language("en", "zh-CN"), Lang::En);
        assert_eq!(resolve_language("zh-Hans", "en-US"), Lang::ZhHans);
        assert_eq!(resolve_language("EN", "zh-CN"), Lang::En);
        // Unknown/unsupported tags fall back to English (spec §3.10).
        assert_eq!(resolve_language("fr", "zh-CN"), Lang::En);
        assert_eq!(resolve_language("", "zh-CN"), Lang::En);
        assert_eq!(resolve_language("system", ""), Lang::En);
        // `effective_language` is the test seam used by the tray: same contract as `resolve_language`.
        assert_eq!(effective_language("system", "zh-CN"), Lang::ZhHans);
        assert_eq!(effective_language("system", "en-GB"), Lang::En);
    }

    /// `resolved_tag_for` is what the full-state broadcast exposes as `resolvedLanguage`; the tray
    /// and the panel both derive their table from it, so the two halves cannot disagree.
    #[test]
    fn resolved_tag_is_the_single_source_for_tray_and_panel_language() {
        assert_eq!(resolved_tag_for("system", "zh-CN"), "zh-Hans");
        assert_eq!(resolved_tag_for("system", "en-US"), "en");
        assert_eq!(resolved_tag_for("zh-Hans", "en-US"), "zh-Hans");
        assert_eq!(resolved_tag_for("en", "zh-CN"), "en");
        // Unsupported/hand-edited tags fall back to English in both layers.
        assert_eq!(resolved_tag_for("fr", "zh-CN"), "en");
        assert_eq!(resolved_tag_for("", "zh-CN"), "en");
        // The process-wide resolution against the real display language is a supported tag.
        let system = system_language();
        assert!(
            ["en", "zh-Hans"].contains(&resolved_tag("system")),
            "system language tag {system:?} must resolve to a shipped language"
        );
        assert_eq!(resolved_tag("system"), resolved_tag_for("system", system));
    }

    /// The display-language fallback chain: preferred UI language → default UI language → regional
    /// locale → English. This is the Important-2 fix for mixed-locale Windows installs where the
    /// regional format locale (`GetUserDefaultLocaleName`) differs from the display language.
    #[test]
    fn system_language_prefers_display_language_then_falls_back() {
        // Preferred display language wins even when the regional locale is English.
        assert_eq!(
            pick_system_language(Some("zh-CN"), Some("en"), Some("en-US")),
            "zh-CN"
        );
        // Mixed locale: regional format zh-CN but display language en-US -> English display wins.
        assert_eq!(
            pick_system_language(Some("en-US"), Some("en"), Some("zh-CN")),
            "en-US"
        );
        // No preferred list: the default UI language LANGID mapping is next.
        assert_eq!(pick_system_language(None, Some("zh-Hans"), Some("en-US")), "zh-Hans");
        // No UI-language APIs: the regional locale is the last useful fallback.
        assert_eq!(pick_system_language(None, None, Some("zh-TW")), "zh-TW");
        // Nothing usable: English (spec §3.10 fallback).
        assert_eq!(pick_system_language(None, None, None), "en");
        assert_eq!(pick_system_language(Some("  "), Some(""), Some("\t")), "en");
        // End-to-end through the resolver: a Chinese display language maps to zh-Hans regardless
        // of what the regional format says; English display + Chinese format stays English.
        assert_eq!(
            resolved_tag_for("system", &pick_system_language(Some("zh-CN"), None, Some("en-US"))),
            "zh-Hans"
        );
        assert_eq!(
            resolved_tag_for("system", &pick_system_language(Some("en-US"), None, Some("zh-CN"))),
            "en"
        );
    }

    #[test]
    fn tags_match_the_menu_settings_values() {
        assert_eq!(Lang::En.tag(), "en");
        assert_eq!(Lang::ZhHans.tag(), "zh-Hans");
        let strings = tray_strings(Lang::ZhHans);
        assert_eq!(strings.show_line, "显示线");
        assert_eq!(tray_strings(Lang::En).show_line, "Show line");
    }

    /// W1.7: the dialog formatters fill every placeholder, and both frozen tables own the new rows
    /// (the parity/all() test above already guarantees no missing or blank entry).
    #[test]
    fn w1_7_dialog_formatters_fill_placeholders() {
        for strings in [tray_strings(Lang::En), tray_strings(Lang::ZhHans)] {
            let body = strings.take_all_down_body(3);
            assert!(body.contains('3'), "count must be filled in: {body:?}");
            assert!(
                !body.contains("{}"),
                "count placeholder must be replaced: {body:?}"
            );
            let line = strings.hotkey_error_detail_line("Capture", "Ctrl+Alt+S", "busy");
            assert_eq!(
                line.matches("{}").count(),
                0,
                "detail placeholders must be replaced: {line:?}"
            );
            assert!(line.contains("Capture"), "got {line:?}");
            assert!(line.contains("Ctrl+Alt+S"), "got {line:?}");
            assert!(line.contains("busy"), "got {line:?}");
        }
        assert_eq!(EN.shortcut_edit_latest, "Edit latest");
        assert_eq!(ZH_HANS.shortcut_edit_latest, "编辑最新一张");
        assert!(!EN.keep_info.is_empty() && !ZH_HANS.keep_info.is_empty());
        assert!(!EN.keep_for_info.is_empty() && !ZH_HANS.keep_for_info.is_empty());
    }

    /// Wave 2: the search tray item, shortcut row and window title exist in both frozen tables
    /// (the parity test above covers the key set/blank rule).
    #[test]
    fn w2_search_strings_are_translated() {
        assert_eq!(EN.search, "Search…");
        assert_eq!(ZH_HANS.search, "搜索…");
        assert_eq!(EN.shortcut_search, "Search");
        assert_eq!(ZH_HANS.shortcut_search, "搜索");
        assert_eq!(EN.search_window_title, "Hangline — Search");
        assert_eq!(ZH_HANS.search_window_title, "Hangline — 搜索");
    }

    /// v1.4: the appearance submenu is fully translated in both frozen tables — every rope style
    /// and rope colour has a non-empty label, and the three menu titles match the contract.
    #[test]
    fn v1_4_appearance_strings_are_translated() {
        assert_eq!(EN.appearance_menu, "Appearance");
        assert_eq!(ZH_HANS.appearance_menu, "外观");
        assert_eq!(EN.rope_style_menu, "Rope style");
        assert_eq!(ZH_HANS.rope_style_menu, "绳子样式");
        assert_eq!(EN.rope_color_menu, "Rope color");
        assert_eq!(ZH_HANS.rope_color_menu, "绳子颜色");
        assert_eq!(EN.rope_style_silk, "Silk");
        assert_eq!(ZH_HANS.rope_style_silk, "真丝");
        assert_eq!(EN.rope_style_linen, "Linen");
        assert_eq!(ZH_HANS.rope_style_linen, "亚麻");
        assert_eq!(EN.rope_style_brass, "Brass");
        assert_eq!(ZH_HANS.rope_style_brass, "黄铜链");
        assert_eq!(EN.rope_style_aurora, "Aurora");
        assert_eq!(ZH_HANS.rope_style_aurora, "暮光线");
        assert_eq!(EN.rope_color_champagne, "Champagne");
        assert_eq!(ZH_HANS.rope_color_champagne, "香槟金");
        assert_eq!(EN.rope_color_terracotta, "Terracotta");
        assert_eq!(ZH_HANS.rope_color_terracotta, "赤陶");
        assert_eq!(EN.rope_color_haze, "Haze");
        assert_eq!(ZH_HANS.rope_color_haze, "雾霾蓝");
        // The eight colour labels and four style labels are all present/non-empty in both tables
        // (the parity/all() test above guarantees the key set; this locks the exact 4 + 8 rows).
        let en = EN.all();
        let zh = ZH_HANS.all();
        let style_keys = [
            "rope_style_silk",
            "rope_style_linen",
            "rope_style_brass",
            "rope_style_aurora",
        ];
        let color_keys = [
            "rope_color_ink",
            "rope_color_ivory",
            "rope_color_champagne",
            "rope_color_fog",
            "rope_color_sage",
            "rope_color_haze",
            "rope_color_terracotta",
            "rope_color_violet",
        ];
        for rows in [&en, &zh] {
            for key in style_keys.iter().chain(color_keys.iter()) {
                assert!(
                    rows.iter()
                        .any(|(candidate, value)| candidate == key && !value.is_empty()),
                    "missing appearance label {key}"
                );
            }
            assert_eq!(
                rows.iter()
                    .filter(|(candidate, _)| style_keys.contains(candidate))
                    .count(),
                4
            );
            assert_eq!(
                rows.iter()
                    .filter(|(candidate, _)| color_keys.contains(candidate))
                    .count(),
                8
            );
        }
    }
}
