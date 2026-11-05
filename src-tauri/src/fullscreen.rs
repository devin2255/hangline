//! Full-screen suppression (design spec §3.2 "全屏抑制", §4.6, M4).
//!
//! The line must stay out of the way of a full-screen game, video or presentation: when one is
//! running, the reveal controller is told to treat the line as "cannot show" (it tucks a visible
//! line and defers new-item peeks), and it restores the pre-suppression visibility when the
//! full-screen app goes away (spec M4: back within 1 s).
//!
//! # Detection
//!
//! Two independent probes feed the decision:
//!
//! - [`query_notification_state`] wraps `SHQueryUserNotificationState`. The spec suppresses on
//!   `QUNS_BUSY` (2), `QUNS_RUNNING_D3D_FULL_SCREEN` (3) and `QUNS_PRESENTATION_MODE` (4).
//! - [`foreground_covers_monitor`] checks whether the foreground window's `GetWindowRect` covers
//!   its monitor's full rectangle (a borderless full-screen window, including D3D games that the
//!   shell does not classify). Windows of our own process (the panel, the capture overlay, the
//!   editors), invisible windows, zero-area rects and shell surfaces (`Progman`, `WorkerW`,
//!   `Shell_TrayWnd`, the shell's `Windows.UI.Core.CoreWindow`) are ignored. The window must also
//!   be borderless and not maximized ([`window_is_fullscreen`]): the *monitor* rect is compared,
//!   not the work area, and on a monitor without a visible taskbar a merely maximized window would
//!   otherwise cover the monitor exactly and hide the line. Ordinary maximized windows never
//!   suppress.
//!
//! # Debounce
//!
//! A *change* in the decision is applied only after it has been stable for [`STABLE_MS`] (0.8 s,
//! spec §3.2 "Space 切换/前台应用变化后延迟 0.8 s 复检"). Only the two decision bits
//! ([`GateBits`]) are debounced, never the raw QUNS value, so a shell that alternates between two
//! suppressing values (`QUNS_BUSY` ↔ `QUNS_RUNNING_D3D_FULL_SCREEN`) cannot restart the window.
//! The first observation of a process is applied immediately: there is no prior state to debounce
//! against, and waiting would let the line flash over a full-screen app during startup. The
//! `pinned` / `showOverFullScreen` override is re-evaluated on every poll (not debounced), so
//! changing the setting takes effect within one poll interval.
//!
//! # Fail-safe
//!
//! A failed probe never suppresses: `SHQueryUserNotificationState` errors are logged once per
//! failing streak and the foreground probe alone decides, while a panicking probe is treated as
//! `Normal`. The poller thread itself is wrapped in `catch_unwind`; if it ever dies it clears the
//! suppression and tells the reveal controller, so the line can never stay hidden forever.
//!
//! The poller runs on its own named thread ([`THREAD_NAME`]) every [`POLL_INTERVAL_MS`] (100 ms).
//! A change is applied after [`STABLE_MS`] of stability, measured from the poll that first saw it,
//! so the worst case is 100 ms + 800 ms = 0.9 s: the M4 "back within 1 s" restore bound holds even
//! when the full-screen app goes away immediately after a poll. It emits
//! `fullscreen-state { suppressed, reason? }` whenever the applied state changes (debug/evidence
//! only; no webview listens to it) and posts [`reveal::set_fullscreen_suppressed`] to the single
//! reveal controller, which owns every window transition.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::Shell::SHQueryUserNotificationState;
use windows::Win32::UI::WindowsAndMessaging::{
    GetClassNameW, GetForegroundWindow, GetWindowLongW, GetWindowPlacement, GetWindowRect,
    GetWindowThreadProcessId, IsWindowVisible, GWL_STYLE, SW_SHOWMAXIMIZED, WINDOWPLACEMENT,
    WS_CAPTION, WS_THICKFRAME,
};

use crate::app_state::AppState;
use crate::geometry::PhysicalRect;
use crate::reveal;

/// Poll cadence of the full-screen thread (100 ms).
///
/// The M4 restore bound is one poll plus [`STABLE_MS`]: 100 ms + 800 ms = 0.9 s worst case, so a
/// full-screen app that disappears immediately after a poll still restores the line within 1 s.
pub const POLL_INTERVAL_MS: u64 = 100;

/// How long a changed observation must stay stable before it is applied (spec §3.2: 0.8 s).
pub const STABLE_MS: u64 = 800;

/// `QUNS_BUSY`: a full-screen application is running, or Presentation Settings are on.
pub const QUNS_BUSY: u32 = 2;

/// `QUNS_RUNNING_D3D_FULL_SCREEN`: an exclusive-mode Direct3D application is running.
pub const QUNS_RUNNING_D3D_FULL_SCREEN: u32 = 3;

/// `QUNS_PRESENTATION_MODE`: the user activated Presentation Settings.
pub const QUNS_PRESENTATION_MODE: u32 = 4;

/// Pixels of slack when comparing the foreground window with its monitor rectangle.
///
/// A borderless full-screen window is normally an exact match; the tolerance absorbs a one-pixel
/// DWM rounding without ever accepting a maximized window (which is a whole taskbar short).
pub const COVER_TOLERANCE_PX: i32 = 2;

/// Shell/desktop window classes that are never treated as a full-screen application.
///
/// `Progman`/`WorkerW` are the desktop, `Shell_TrayWnd` is the taskbar and
/// `Windows.UI.Core.CoreWindow` is the shell's Start/Search surface (UWP apps are hosted by
/// `ApplicationFrameWindow`, so excluding the bare CoreWindow class does not hide UWP full-screen
/// content from the foreground probe).
pub const SHELL_CLASSES: &[&str] = &[
    "Progman",
    "WorkerW",
    "Shell_TrayWnd",
    "Windows.UI.Core.CoreWindow",
];

/// Thread name of the poller (shows up in crash/log dumps).
pub const THREAD_NAME: &str = "hangline-fullscreen";

/// Last applied suppression state, readable without a controller round-trip.
///
/// `report_panel_ready` and the READY fallback use this to avoid flashing the panel over a
/// full-screen app during startup; the reveal controller remains the owner of the state machine.
static SUPPRESSED: AtomicBool = AtomicBool::new(false);

/// Why the line is suppressed (serialized camelCase for the `fullscreen-state` event and logs).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SuppressReason {
    /// `SHQueryUserNotificationState` returned one of the suppressing QUNS values.
    QueryUserNotificationState(u32),
    /// The foreground window covers its monitor's full rectangle.
    ForegroundCoversMonitor,
}

impl SuppressReason {
    /// Stable name used in log lines.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::QueryUserNotificationState(_) => "queryUserNotificationState",
            Self::ForegroundCoversMonitor => "foregroundCoversMonitor",
        }
    }
}

/// The two states of the full-screen gate (design spec §3.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum FullscreenState {
    /// No full-screen app: every reveal path is allowed.
    Normal,
    /// A full-screen app owns the screen: reveal paths are gated.
    Suppressed(SuppressReason),
}

impl FullscreenState {
    /// Whether the line must stay hidden.
    pub fn is_suppressed(self) -> bool {
        matches!(self, Self::Suppressed(_))
    }

    /// The suppression reason, when suppressed.
    pub fn reason(self) -> Option<SuppressReason> {
        match self {
            Self::Normal => None,
            Self::Suppressed(reason) => Some(reason),
        }
    }
}

/// Serialized payload of the debug `fullscreen-state` event.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FullscreenEvent {
    /// Whether the reveal controller has been told to suppress the line.
    pub suppressed: bool,
    /// Why, when `suppressed` is `true`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<SuppressReason>,
}

impl From<FullscreenState> for FullscreenEvent {
    fn from(state: FullscreenState) -> Self {
        Self {
            suppressed: state.is_suppressed(),
            reason: state.reason(),
        }
    }
}

/// One raw observation of the two probes (pure, cheap to debounce).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Observation {
    /// `SHQueryUserNotificationState` result (`0` when the probe failed; never suppressing).
    query_state: u32,
    /// Whether the foreground window covers its monitor.
    foreground_covers: bool,
}

impl Observation {
    /// The fail-safe observation: no full-screen app can be concluded from it.
    const fn normal() -> Self {
        Self {
            query_state: 0,
            foreground_covers: false,
        }
    }

    /// The two decision bits this observation contributes to the debounce.
    fn bits(self) -> GateBits {
        GateBits {
            quns_suppresses: is_suppressing_quns(self.query_state),
            foreground_covers: self.foreground_covers,
        }
    }
}

/// The two decision bits of one observation (what the debounce compares).
///
/// The raw QUNS value is deliberately not part of it: `QUNS_BUSY` (2) and
/// `QUNS_RUNNING_D3D_FULL_SCREEN` (3) both mean "suppress", so a shell that alternates between
/// them must not restart the 0.8 s stability window. The value is still remembered for the log and
/// event reason (see [`debounced_reason`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct GateBits {
    /// `SHQueryUserNotificationState` returned one of the suppressing values.
    quns_suppresses: bool,
    /// The foreground window covers its monitor as a full-screen surface.
    foreground_covers: bool,
}

/// Whether `query_state` is one of the spec's suppressing QUNS values.
pub fn is_suppressing_quns(query_state: u32) -> bool {
    matches!(
        query_state,
        QUNS_BUSY | QUNS_RUNNING_D3D_FULL_SCREEN | QUNS_PRESENTATION_MODE
    )
}

/// The raw suppression reason of one probe pair, before the user override is applied.
///
/// `QUNS` takes precedence over the foreground probe so a D3D full-screen game is reported with
/// its QUNS value (the more specific signal) even though both branches would suppress.
pub fn suppression_reason(
    query_state: u32,
    foreground_covers: bool,
) -> Option<SuppressReason> {
    if is_suppressing_quns(query_state) {
        Some(SuppressReason::QueryUserNotificationState(query_state))
    } else if foreground_covers {
        Some(SuppressReason::ForegroundCoversMonitor)
    } else {
        None
    }
}

/// The spec's decision table: suppress unless the user pinned the line **and** explicitly enabled
/// "show over full-screen apps" (design spec §3.2).
pub fn should_suppress(
    query_state: u32,
    foreground_covers: bool,
    pinned: bool,
    show_over_full_screen: bool,
) -> Option<SuppressReason> {
    apply_override(
        suppression_reason(query_state, foreground_covers),
        pinned,
        show_over_full_screen,
    )
}

/// Applies the user's explicit override to a raw suppression reason.
///
/// Both the pin and the "show over full-screen apps" setting are required; either one alone still
/// suppresses (design spec §3.2).
fn apply_override(
    reason: Option<SuppressReason>,
    pinned: bool,
    show_over_full_screen: bool,
) -> Option<SuppressReason> {
    let reason = reason?;
    if pinned && show_over_full_screen {
        None
    } else {
        Some(reason)
    }
}

/// The suppression reason of the *debounced* decision, using `query_state` only as the reported
/// QUNS detail when the QUNS bit is set.
///
/// The debounce compares [`GateBits`], so this keeps the applied decision and the reported reason
/// consistent even when the raw QUNS value flickered between two suppressing values while the
/// candidate was stable. `query_state` is the last suppressing value seen (a QUNS bit can only be
/// set if one was observed, so the caller always has a meaningful value).
fn debounced_reason(bits: GateBits, query_state: u32) -> Option<SuppressReason> {
    if bits.quns_suppresses {
        Some(SuppressReason::QueryUserNotificationState(query_state))
    } else if bits.foreground_covers {
        Some(SuppressReason::ForegroundCoversMonitor)
    } else {
        None
    }
}

/// Pure classifier for windows that must never count as a full-screen app.
///
/// `same_process` covers the panel, the fly/capture overlays and both editor windows: Hangline's
/// own topmost windows must not suppress the line. The shell classes are filtered by exact class
/// name (Win32 class names are case-sensitive).
pub fn foreground_class_is_own_or_shell(class_name: &str, same_process: bool) -> bool {
    same_process || SHELL_CLASSES.contains(&class_name)
}

/// Returns whether a top-level window looks like a full-screen surface.
///
/// A full-screen window is borderless: it has neither a title bar (`WS_CAPTION`) nor a resize
/// frame (`WS_THICKFRAME`). A merely maximized window keeps both style bits (Windows does not strip
/// them when maximizing), so it is rejected even on a monitor without a visible taskbar, where its
/// rectangle covers the monitor exactly. A window whose `GetWindowPlacement.showCmd` is
/// `SW_SHOWMAXIMIZED` is rejected too: a maximized window is managed by the shell (restore
/// animation, work-area rules) even in the rare case it was created borderless, so it is not an
/// exclusive full-screen surface. The rectangle check still has to prove that a borderless window
/// really covers its monitor.
pub fn window_is_fullscreen(style: u32, show_cmd: u32) -> bool {
    let framed = style & (WS_CAPTION.0 | WS_THICKFRAME.0) != 0;
    !framed && show_cmd != SW_SHOWMAXIMIZED.0 as u32
}

/// Returns whether `window` covers `target` on all four sides, allowing `tolerance` pixels of slack.
///
/// Both rectangles are in physical virtual-desktop pixels (negative monitor origins work). An
/// empty window rect covers nothing. This is the pure half of [`foreground_covers_monitor`].
pub fn covers_rect(window: PhysicalRect, target: PhysicalRect, tolerance: i32) -> bool {
    if window.w <= 0 || window.h <= 0 || target.w <= 0 || target.h <= 0 {
        return false;
    }
    let tolerance = tolerance.max(0);
    window.x <= target.x.saturating_add(tolerance)
        && window.y <= target.y.saturating_add(tolerance)
        && window.x.saturating_add(window.w)
            >= target.x.saturating_add(target.w).saturating_sub(tolerance)
        && window.y.saturating_add(window.h)
            >= target.y.saturating_add(target.h).saturating_sub(tolerance)
}

/// Returns whether an observation that started at `since_ms` has been stable at `now_ms`.
///
/// Boundary: exactly [`STABLE_MS`] counts as stable (the spec says "after 0.8 s"), and a clock
/// that moved backwards is never stable.
pub fn stable_for(since_ms: u64, now_ms: u64) -> bool {
    now_ms.saturating_sub(since_ms) >= STABLE_MS
}

/// A debounced value: the first observation is applied immediately, every later change only after
/// [`STABLE_MS`] of stability. Reverting to the applied value cancels the pending change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Debounce<T> {
    /// The last applied value (`None` before the first observation).
    applied: Option<T>,
    /// The candidate waiting for the stability window.
    candidate: Option<T>,
    /// First observation time of the candidate, in controller milliseconds.
    since: u64,
}

impl<T> Default for Debounce<T> {
    fn default() -> Self {
        Self {
            applied: None,
            candidate: None,
            since: 0,
        }
    }
}

impl<T: Copy + PartialEq> Debounce<T> {
    /// Feeds one observation; returns the value to apply now, if any.
    fn observe(&mut self, observed: T, now: u64) -> Option<T> {
        let Some(applied) = self.applied else {
            // First observation of the process: no prior state to debounce against, and waiting
            // would flash the line over a full-screen app during startup.
            self.applied = Some(observed);
            self.candidate = None;
            return Some(observed);
        };
        if observed == applied {
            self.candidate = None;
            return None;
        }
        if self.candidate != Some(observed) {
            self.candidate = Some(observed);
            self.since = now;
        }
        if stable_for(self.since, now) {
            self.applied = Some(observed);
            self.candidate = None;
            return Some(observed);
        }
        None
    }

    /// When the pending candidate becomes stable, if one is waiting.
    fn pending_deadline(&self) -> Option<u64> {
        self.candidate
            .map(|_| self.since.saturating_add(STABLE_MS))
    }
}

/// Wraps `SHQueryUserNotificationState` (design spec §3.2).
///
/// The `windows` crate models the result as `QUERY_USER_NOTIFICATION_STATE(i32)`; a negative value
/// can never match a suppressing QUNS constant and is clamped to `0` (normal).
pub fn query_notification_state() -> windows::core::Result<u32> {
    // SAFETY: SHQueryUserNotificationState writes the state into the value it returns and takes no
    // other input.
    let state = unsafe { SHQueryUserNotificationState() }?;
    Ok(u32::try_from(state.0).unwrap_or(0))
}

/// Returns whether the foreground window covers its monitor's full rectangle.
///
/// Excluded on purpose (see [`foreground_class_is_own_or_shell`], [`window_is_fullscreen`] and the
/// module docs): Hangline's own windows, invisible windows, zero-area windows, shell/desktop
/// surfaces and ordinary framed/maximized windows. The comparison uses the monitor rectangle (not
/// the work area) with [`COVER_TOLERANCE_PX`] slack; the style/placement check is what keeps a
/// merely maximized window on a taskbar-less monitor from suppressing the line.
pub fn foreground_covers_monitor() -> bool {
    // SAFETY: every call below is a read-only query; the RECT/MONITORINFO/WINDOWPLACEMENT
    // out-parameters are stack values that outlive their calls.
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.is_invalid() {
        return false;
    }

    let mut process_id = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(std::ptr::from_mut(&mut process_id))) };
    // SAFETY: GetCurrentProcessId has no arguments and cannot fail.
    let same_process = process_id != 0 && process_id == unsafe { GetCurrentProcessId() };

    let mut class_buffer = [0u16; 256];
    let class_len = unsafe { GetClassNameW(hwnd, &mut class_buffer) };
    let class_name = if class_len > 0 {
        String::from_utf16_lossy(&class_buffer[..usize::try_from(class_len).unwrap_or(0)])
    } else {
        String::new()
    };
    if foreground_class_is_own_or_shell(&class_name, same_process) {
        return false;
    }

    if !unsafe { IsWindowVisible(hwnd) }.as_bool() {
        return false;
    }

    let style = unsafe { GetWindowLongW(hwnd, GWL_STYLE) } as u32;
    let mut placement = WINDOWPLACEMENT {
        length: u32::try_from(std::mem::size_of::<WINDOWPLACEMENT>()).unwrap_or(0),
        ..WINDOWPLACEMENT::default()
    };
    // A failed placement query is treated as "not maximized": the style bits (borderless) are the
    // primary proof, and a probe failure must never invent a suppression.
    let show_cmd = match unsafe { GetWindowPlacement(hwnd, std::ptr::from_mut(&mut placement)) } {
        Ok(()) => placement.showCmd,
        Err(_) => 0,
    };
    if !window_is_fullscreen(style, show_cmd) {
        return false;
    }

    let mut window = RECT::default();
    if unsafe { GetWindowRect(hwnd, std::ptr::from_mut(&mut window)) }.is_err() {
        return false;
    }
    if window.right <= window.left || window.bottom <= window.top {
        return false;
    }

    let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    if monitor.is_invalid() {
        return false;
    }
    let mut info = MONITORINFO {
        cbSize: u32::try_from(std::mem::size_of::<MONITORINFO>()).unwrap_or(0),
        ..MONITORINFO::default()
    };
    if !unsafe { GetMonitorInfoW(monitor, std::ptr::from_mut(&mut info)) }.as_bool() {
        return false;
    }

    covers_rect(
        PhysicalRect {
            x: window.left,
            y: window.top,
            w: window.right.saturating_sub(window.left),
            h: window.bottom.saturating_sub(window.top),
        },
        PhysicalRect {
            x: info.rcMonitor.left,
            y: info.rcMonitor.top,
            w: info.rcMonitor.right.saturating_sub(info.rcMonitor.left),
            h: info.rcMonitor.bottom.saturating_sub(info.rcMonitor.top),
        },
        COVER_TOLERANCE_PX,
    )
}

/// Probes both inputs once and returns the full-screen gate as it is *right now*.
///
/// The reveal controller calls this when a capture overlay ends: the overlay is one of Hangline's
/// own windows, so [`foreground_covers_monitor`] excludes it and the poller can only see "normal"
/// while it covers a still-full-screen app. A fresh probe after the overlay is gone is therefore
/// the only way to know whether the line may reappear. This does not touch the poller's debounce or
/// its applied state; the poller remains the long-term owner of the gate.
///
/// A failed or panicking probe falls back to [`Observation::normal`] exactly like the poller.
pub fn probe_state(app: &AppHandle) -> FullscreenState {
    let (query_state, foreground_covers) =
        match std::panic::catch_unwind(|| (query_notification_state(), foreground_covers_monitor()))
        {
            Ok((query, covers)) => (query.unwrap_or(0), covers),
            Err(_) => {
                log::error!(
                    "fullscreen: the capture-resume probe panicked; treating it as normal (fail-safe)"
                );
                (0, false)
            }
        };
    let pinned = reveal::is_pinned();
    let show_over_full_screen = app
        .try_state::<AppState>()
        .is_some_and(|state| state.settings().show_over_full_screen);
    match should_suppress(query_state, foreground_covers, pinned, show_over_full_screen) {
        Some(reason) => FullscreenState::Suppressed(reason),
        None => FullscreenState::Normal,
    }
}

/// Milliseconds since the poller started; explicit in every log line for timing evidence.
fn millis(origin: Instant) -> u64 {
    u64::try_from(origin.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// Polls both probes once. Errors are logged once per failing streak; a panic is treated as normal.
fn probe_tick(quns_error_logged: &mut bool, panic_logged: &mut bool) -> Observation {
    // A wedged probe must never unwind the poller thread (design plan: "panics must not leave the
    // flag stuck"). The closure captures nothing, so it is trivially unwind-safe.
    match std::panic::catch_unwind(|| (query_notification_state(), foreground_covers_monitor())) {
        Ok((query, covers)) => {
            let query_state = match query {
                Ok(value) => {
                    if *quns_error_logged {
                        log::info!("fullscreen: SHQueryUserNotificationState recovered");
                        *quns_error_logged = false;
                    }
                    value
                }
                Err(error) => {
                    if !*quns_error_logged {
                        log::warn!(
                            "fullscreen: SHQueryUserNotificationState failed ({error}); the \
                             foreground probe alone decides until it recovers"
                        );
                        *quns_error_logged = true;
                    }
                    // 0 never matches a suppressing QUNS value: the failing branch cannot suppress.
                    0
                }
            };
            Observation {
                query_state,
                foreground_covers: covers,
            }
        }
        Err(_) => {
            if !*panic_logged {
                log::error!(
                    "fullscreen: the probe panicked; treating the tick as normal (fail-safe)"
                );
                *panic_logged = true;
            }
            Observation::normal()
        }
    }
}

/// Sleeps `total_ms` in short steps so [`shutdown`]/[`FullscreenPoller::stop`] is honoured quickly.
fn sleep_unless_stopped(stop: &AtomicBool, total_ms: u64) {
    let mut remaining = total_ms;
    while remaining > 0 {
        if stop.load(Ordering::Acquire) {
            return;
        }
        let step = remaining.min(50);
        thread::sleep(Duration::from_millis(step));
        remaining -= step;
    }
}

/// The poller loop: probe, debounce, apply changes to [`reveal`] only when the gate flips.
fn poll_loop(app: &AppHandle, stop: &AtomicBool) {
    let origin = Instant::now();
    let mut debounce: Debounce<GateBits> = Debounce::default();
    let mut stable = GateBits::default();
    // The last suppressing QUNS value, kept only for the reason of a debounced QUNS decision.
    let mut suppressing_quns = QUNS_BUSY;
    let mut applied = false;
    let mut quns_error_logged = false;
    let mut probe_panic_logged = false;
    let mut first_observation = true;

    loop {
        if stop.load(Ordering::Acquire) {
            log::info!("fullscreen: poller stopped at +{} ms", millis(origin));
            break;
        }
        let Some(settings) = app
            .try_state::<AppState>()
            .map(|state| state.settings())
        else {
            log::info!("fullscreen: application state is gone; stopping the poller");
            break;
        };
        let now = millis(origin);

        let observation = probe_tick(&mut quns_error_logged, &mut probe_panic_logged);
        if first_observation {
            first_observation = false;
            log::info!(
                "fullscreen: poller started (interval {POLL_INTERVAL_MS} ms, stable {STABLE_MS} ms); \
                 first observation quns={}, foreground_covers={}",
                observation.query_state,
                observation.foreground_covers
            );
        }
        if is_suppressing_quns(observation.query_state) {
            suppressing_quns = observation.query_state;
        }
        if let Some(next) = debounce.observe(observation.bits(), now) {
            stable = next;
        }

        // The user override is re-evaluated every poll from live state: pinning the line or
        // enabling `showOverFullScreen` must take effect within one poll interval. The debounced
        // decision bits are authoritative; the raw QUNS value only labels the QUNS reason.
        let reason = apply_override(
            debounced_reason(stable, suppressing_quns),
            reveal::is_pinned(),
            settings.show_over_full_screen,
        );
        let fullscreen = match reason {
            Some(reason) => FullscreenState::Suppressed(reason),
            None => FullscreenState::Normal,
        };
        let next_suppressed = fullscreen.is_suppressed();
        if next_suppressed != applied {
            applied = next_suppressed;
            SUPPRESSED.store(next_suppressed, Ordering::Release);
            match fullscreen.reason() {
                Some(reason) => log::info!(
                    "fullscreen: suppression on at +{now} ms (reason {}, quns={}, \
                     foreground_covers={})",
                    reason.as_str(),
                    observation.query_state,
                    observation.foreground_covers
                ),
                None => log::info!(
                    "fullscreen: suppression off at +{now} ms (normal; pinned={}, \
                     showOverFullScreen={})",
                    reveal::is_pinned(),
                    settings.show_over_full_screen
                ),
            }
            let event = FullscreenEvent::from(fullscreen);
            if let Err(error) = app.emit("fullscreen-state", event) {
                log::warn!("fullscreen: cannot emit fullscreen-state: {error}");
            }
            reveal::set_fullscreen_suppressed(app, next_suppressed);
        }

        // Wake at the next regular poll, or exactly when a pending change becomes stable.
        let wait = match debounce.pending_deadline() {
            Some(deadline) => deadline
                .saturating_sub(now)
                .min(POLL_INTERVAL_MS)
                .max(1),
            None => POLL_INTERVAL_MS,
        };
        sleep_unless_stopped(stop, wait);
    }
}

/// Handle to the full-screen poller; managed by Tauri so shutdown can stop the thread.
pub struct FullscreenPoller {
    stop: Arc<AtomicBool>,
}

impl FullscreenPoller {
    /// Signals the poller thread to exit at its next wake-up.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Release);
    }
}

impl Drop for FullscreenPoller {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Returns whether the last applied full-screen gate says "suppressed".
pub fn is_suppressed() -> bool {
    SUPPRESSED.load(Ordering::Acquire)
}

/// Starts the full-screen poller thread (once per process; `lib.rs` manages the returned handle).
///
/// The thread probes every [`POLL_INTERVAL_MS`], applies a changed observation after [`STABLE_MS`]
/// and calls [`reveal::set_fullscreen_suppressed`] only when the gate flips. A panic anywhere in
/// the loop clears the gate before the thread ends so the line can never stay hidden forever.
pub fn start(app: AppHandle) -> FullscreenPoller {
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = Arc::clone(&stop);
    let thread_app = app.clone();

    let spawned = thread::Builder::new()
        .name(THREAD_NAME.to_string())
        .spawn(move || {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                poll_loop(&thread_app, &thread_stop);
            }));
            if outcome.is_err() {
                log::error!(
                    "fullscreen: the poller thread panicked; clearing suppression so the line can \
                     never stay hidden (fail-safe)"
                );
                SUPPRESSED.store(false, Ordering::Release);
                reveal::set_fullscreen_suppressed(&thread_app, false);
            }
        });
    if let Err(error) = spawned {
        // Without a poller the app simply behaves normally (no suppression), never stuck.
        log::error!("fullscreen: cannot spawn the poller thread: {error}");
    }
    FullscreenPoller { stop }
}

/// Stops the poller at app exit (called from the `RunEvent::Exit` handler).
pub fn shutdown(app: &AppHandle) {
    if let Some(poller) = app.try_state::<FullscreenPoller>() {
        poller.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The public QUNS constants must match the `windows` crate exactly (no guessed values).
    #[test]
    fn quns_constants_match_the_windows_crate() {
        use windows::Win32::UI::Shell::{
            QUNS_BUSY as WINDOWS_BUSY, QUNS_PRESENTATION_MODE as WINDOWS_PRESENTATION_MODE,
            QUNS_RUNNING_D3D_FULL_SCREEN as WINDOWS_D3D,
        };
        assert_eq!(WINDOWS_BUSY.0, QUNS_BUSY as i32);
        assert_eq!(WINDOWS_D3D.0, QUNS_RUNNING_D3D_FULL_SCREEN as i32);
        assert_eq!(
            WINDOWS_PRESENTATION_MODE.0,
            QUNS_PRESENTATION_MODE as i32
        );
        assert_eq!(
            (QUNS_BUSY, QUNS_RUNNING_D3D_FULL_SCREEN, QUNS_PRESENTATION_MODE),
            (2, 3, 4)
        );
    }

    /// Only the three spec values suppress; every other documented QUNS value is normal.
    #[test]
    fn only_busy_d3d_and_presentation_suppress() {
        let suppressing = [QUNS_BUSY, QUNS_RUNNING_D3D_FULL_SCREEN, QUNS_PRESENTATION_MODE];
        for value in 0..=10u32 {
            assert_eq!(
                is_suppressing_quns(value),
                suppressing.contains(&value),
                "QUNS {value}"
            );
        }
        assert!(!is_suppressing_quns(0), "0 is the probe-failure fallback");
        assert!(!is_suppressing_quns(1), "QUNS_NOT_PRESENT");
        assert!(!is_suppressing_quns(5), "QUNS_ACCEPTS_NOTIFICATIONS");
        assert!(!is_suppressing_quns(6), "QUNS_QUIET_TIME");
        assert!(!is_suppressing_quns(7), "QUNS_APP");
    }

    /// The full matrix: reason, foreground flag and the pinned/override escape hatch.
    #[test]
    fn should_suppress_matrix() {
        // Plain cases, no override.
        assert_eq!(should_suppress(0, false, false, false), None);
        assert_eq!(should_suppress(5, false, false, false), None);
        assert_eq!(
            should_suppress(QUNS_BUSY, false, false, false),
            Some(SuppressReason::QueryUserNotificationState(QUNS_BUSY))
        );
        assert_eq!(
            should_suppress(QUNS_RUNNING_D3D_FULL_SCREEN, false, false, true),
            Some(SuppressReason::QueryUserNotificationState(
                QUNS_RUNNING_D3D_FULL_SCREEN
            ))
        );
        assert_eq!(
            should_suppress(QUNS_PRESENTATION_MODE, false, true, false),
            Some(SuppressReason::QueryUserNotificationState(
                QUNS_PRESENTATION_MODE
            ))
        );
        assert_eq!(
            should_suppress(0, true, false, false),
            Some(SuppressReason::ForegroundCoversMonitor)
        );

        // QUNS wins over the foreground flag (the more specific reason is reported).
        assert_eq!(
            should_suppress(QUNS_BUSY, true, false, false),
            Some(SuppressReason::QueryUserNotificationState(QUNS_BUSY))
        );

        // The explicit override requires *both* a pinned line and the setting.
        assert_eq!(should_suppress(QUNS_BUSY, false, true, true), None);
        assert_eq!(should_suppress(0, true, true, true), None);
        assert_eq!(
            should_suppress(QUNS_BUSY, false, true, false),
            Some(SuppressReason::QueryUserNotificationState(QUNS_BUSY)),
            "pinned alone must not override"
        );
        assert_eq!(
            should_suppress(QUNS_BUSY, false, false, true),
            Some(SuppressReason::QueryUserNotificationState(QUNS_BUSY)),
            "the setting alone must not override"
        );

        // An override on a normal observation stays normal.
        assert_eq!(should_suppress(0, false, true, true), None);
    }

    /// Own-process and shell windows are exactly the ones the foreground probe ignores.
    #[test]
    fn foreground_classifier_ignores_own_process_and_shell_surfaces() {
        assert!(foreground_class_is_own_or_shell("HanglinePanel", true));
        for class in SHELL_CLASSES {
            assert!(
                foreground_class_is_own_or_shell(class, false),
                "{class} is a shell surface"
            );
        }
        assert!(!foreground_class_is_own_or_shell("Chrome_WidgetWin_1", false));
        assert!(!foreground_class_is_own_or_shell(
            "WindowsForms10.Window.8.app.0.141b42a_r6_ad1",
            false
        ));
        assert!(!foreground_class_is_own_or_shell("ApplicationFrameWindow", false));
        // Win32 class names are case-sensitive; a near miss is a normal app.
        assert!(!foreground_class_is_own_or_shell("progman", false));
        assert!(!foreground_class_is_own_or_shell("", false));
    }

    /// A framed or maximized window is never a full-screen surface, even when it covers the
    /// monitor rectangle; a borderless, non-maximized window is a candidate.
    #[test]
    fn only_borderless_non_maximized_windows_are_fullscreen() {
        use windows::Win32::UI::WindowsAndMessaging::{SW_SHOWNORMAL, WS_POPUP, WS_VISIBLE};

        // A normal resizable window (`WS_OVERLAPPEDWINDOW`: caption + thick frame), maximized or
        // not. This is the taskbar-less-monitor false positive: it covers the monitor rectangle
        // exactly but must not suppress the line.
        let overlapped = WS_CAPTION.0 | WS_THICKFRAME.0 | WS_VISIBLE.0 | 0x0002_0000;
        assert!(!window_is_fullscreen(overlapped, SW_SHOWMAXIMIZED.0 as u32));
        assert!(!window_is_fullscreen(overlapped, SW_SHOWNORMAL.0 as u32));

        // A borderless popup covering the monitor (the synthetic evidence helper, a borderless
        // game or a full-screen video window).
        let popup = WS_POPUP.0 | WS_VISIBLE.0;
        assert!(window_is_fullscreen(popup, SW_SHOWNORMAL.0 as u32));

        // A borderless window created maximized is still a shell-managed maximized window.
        assert!(!window_is_fullscreen(popup, SW_SHOWMAXIMIZED.0 as u32));

        // Either decoration bit alone is enough to reject.
        assert!(!window_is_fullscreen(WS_CAPTION.0, SW_SHOWNORMAL.0 as u32));
        assert!(!window_is_fullscreen(WS_THICKFRAME.0, SW_SHOWNORMAL.0 as u32));

        // A bare borderless window is only a candidate: `covers_rect` still has to prove it
        // covers its monitor.
        assert!(window_is_fullscreen(0, SW_SHOWNORMAL.0 as u32));
        assert!(window_is_fullscreen(WS_POPUP.0, 0));
    }

    /// A full-monitor window covers; a maximized (work-area) or smaller window does not.
    #[test]
    fn covers_rect_requires_the_monitor_not_the_work_area() {
        let monitor = PhysicalRect {
            x: 0,
            y: 0,
            w: 1920,
            h: 1080,
        };
        assert!(covers_rect(monitor, monitor, COVER_TOLERANCE_PX));
        assert!(
            covers_rect(
                PhysicalRect {
                    x: -2,
                    y: -2,
                    w: 1924,
                    h: 1084
                },
                monitor,
                COVER_TOLERANCE_PX
            ),
            "a window a couple of pixels larger still covers the monitor"
        );
        assert!(
            covers_rect(
                PhysicalRect {
                    x: -8,
                    y: -8,
                    w: 1936,
                    h: 1376
                },
                monitor,
                COVER_TOLERANCE_PX
            ),
            "a larger rect still covers"
        );

        // A maximized window stops at the taskbar: 40 px shorter than the monitor.
        let maximized = PhysicalRect {
            x: 0,
            y: 0,
            w: 1920,
            h: 1040,
        };
        assert!(!covers_rect(maximized, monitor, COVER_TOLERANCE_PX));
        // Within tolerance: the tolerance only absorbs rounding, never a taskbar.
        assert!(covers_rect(
            PhysicalRect {
                x: 0,
                y: 0,
                w: 1919,
                h: 1079
            },
            monitor,
            COVER_TOLERANCE_PX
        ));
        assert!(!covers_rect(
            PhysicalRect {
                x: 0,
                y: 0,
                w: 1915,
                h: 1080
            },
            monitor,
            COVER_TOLERANCE_PX
        ));

        // Negative-origin monitors (secondary display left of the primary).
        let left_monitor = PhysicalRect {
            x: -1920,
            y: 0,
            w: 1920,
            h: 1080,
        };
        assert!(covers_rect(left_monitor, left_monitor, 0));
        assert!(!covers_rect(monitor, left_monitor, 0));
        assert!(!covers_rect(
            PhysicalRect {
                x: 0,
                y: 0,
                w: 0,
                h: 1080
            },
            monitor,
            0
        ));
        assert!(!covers_rect(monitor, PhysicalRect { x: 100, y: 100, w: 0, h: 0 }, 0));
    }

    /// The 0.8 s boundary is inclusive and a backwards clock never counts as stable.
    #[test]
    fn stable_for_boundary() {
        assert!(!stable_for(0, 0));
        assert!(!stable_for(0, 799));
        assert!(stable_for(0, 800));
        assert!(stable_for(0, 801));
        assert!(stable_for(1000, 1800));
        assert!(!stable_for(1000, 1799));
        assert!(!stable_for(5000, 100), "a backwards clock is not stable");
    }

    /// The debouncer applies the first observation immediately and later changes only after 0.8 s.
    #[test]
    fn debounce_applies_a_change_only_after_the_stability_window() {
        let mut debounce: Debounce<u32> = Debounce::default();
        assert_eq!(debounce.observe(1, 0), Some(1), "first observation applies");
        assert_eq!(debounce.pending_deadline(), None);

        assert_eq!(debounce.observe(2, 100), None);
        assert_eq!(debounce.pending_deadline(), Some(900));
        assert_eq!(debounce.observe(2, 899), None);
        assert_eq!(debounce.observe(2, 900), Some(2), "exactly 0.8 s applies");

        // A revert to the applied value cancels the pending change instead of flapping.
        assert_eq!(debounce.observe(3, 1000), None);
        assert_eq!(debounce.pending_deadline(), Some(1800));
        assert_eq!(debounce.observe(2, 1100), None);
        assert_eq!(debounce.pending_deadline(), None);
        assert_eq!(debounce.observe(2, 5000), None, "still the applied value");

        // A new candidate restarts the stability window at its first observation.
        assert_eq!(debounce.observe(4, 6000), None);
        assert_eq!(debounce.observe(4, 6500), None);
        assert_eq!(debounce.observe(4, 6800), Some(4));
        assert_eq!(debounce.pending_deadline(), None);
    }

    /// Only the decision bits are debounced: a QUNS value flicker between two suppressing values
    /// must not restart the stability window, while a real decision change still does.
    #[test]
    fn quns_value_flicker_does_not_restart_the_stability_window() {
        let busy = Observation {
            query_state: QUNS_BUSY,
            foreground_covers: true,
        };
        let d3d = Observation {
            query_state: QUNS_RUNNING_D3D_FULL_SCREEN,
            foreground_covers: true,
        };
        assert_eq!(busy.bits(), d3d.bits(), "both values mean the same decision");

        let mut debounce: Debounce<GateBits> = Debounce::default();
        assert_eq!(debounce.observe(busy.bits(), 0), Some(busy.bits()));
        assert_eq!(debounce.pending_deadline(), None);

        // BUSY <-> D3D (and the foreground flag unchanged) is not a change.
        assert_eq!(debounce.observe(d3d.bits(), 300), None);
        assert_eq!(debounce.pending_deadline(), None);
        assert_eq!(debounce.observe(busy.bits(), 700), None);
        assert_eq!(debounce.pending_deadline(), None);

        // A real change (the foreground window stops covering) starts the 0.8 s window.
        let foreground_gone = GateBits {
            quns_suppresses: true,
            foreground_covers: false,
        };
        assert_eq!(debounce.observe(foreground_gone, 1000), None);
        assert_eq!(debounce.pending_deadline(), Some(1800));
        assert_eq!(debounce.observe(foreground_gone, 1800), Some(foreground_gone));
    }

    /// The debounced decision and its reported reason stay consistent, and the QUNS value is only
    /// used as the QUNS reason's detail.
    #[test]
    fn debounced_reason_matches_the_decision_bits() {
        assert_eq!(
            debounced_reason(
                GateBits {
                    quns_suppresses: true,
                    foreground_covers: true,
                },
                QUNS_RUNNING_D3D_FULL_SCREEN
            ),
            Some(SuppressReason::QueryUserNotificationState(
                QUNS_RUNNING_D3D_FULL_SCREEN
            )),
            "QUNS wins the reason precedence"
        );
        assert_eq!(
            debounced_reason(
                GateBits {
                    quns_suppresses: false,
                    foreground_covers: true,
                },
                QUNS_BUSY
            ),
            Some(SuppressReason::ForegroundCoversMonitor)
        );
        assert_eq!(debounced_reason(GateBits::default(), QUNS_BUSY), None);

        // The override is applied after the debounced decision, not inside it.
        let reason = debounced_reason(
            GateBits {
                quns_suppresses: false,
                foreground_covers: true,
            },
            QUNS_BUSY,
        );
        assert_eq!(apply_override(reason, true, true), None);
        assert_eq!(apply_override(reason, true, false), reason);
        assert_eq!(apply_override(reason, false, true), reason);
    }

    /// The event payload is the documented `{ suppressed, reason? }` shape.
    #[test]
    fn fullscreen_event_serializes_camel_case() {
        assert_eq!(
            serde_json::to_value(FullscreenEvent::from(FullscreenState::Normal)).unwrap(),
            serde_json::json!({ "suppressed": false })
        );
        assert_eq!(
            serde_json::to_value(FullscreenEvent::from(FullscreenState::Suppressed(
                SuppressReason::QueryUserNotificationState(QUNS_BUSY)
            )))
            .unwrap(),
            serde_json::json!({
                "suppressed": true,
                "reason": { "queryUserNotificationState": 2 }
            })
        );
        assert_eq!(
            serde_json::to_value(FullscreenState::Suppressed(
                SuppressReason::ForegroundCoversMonitor
            ))
            .unwrap(),
            serde_json::json!({ "suppressed": "foregroundCoversMonitor" })
        );
        assert_eq!(
            serde_json::to_value(FullscreenState::Normal).unwrap(),
            serde_json::json!("normal")
        );
    }
}
