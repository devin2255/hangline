//! Reveal/tuck state machine and the 30 Hz panel controller (design spec §3.4).
//!
//! The panel is not a permanently visible bar: it is summoned by a new item (a timed *peek*), by
//! the toggle hotkey/tray/second launch (*pinned*), or — optionally — by dwelling at the screen's
//! top edge. It tucks away when the pointer leaves the panel+band for 500 ms, when an empty line
//! was revealed, or when the user clicks outside it (which additionally suppresses re-reveals
//! until the pointer leaves the trigger band).
//!
//! # Shape
//!
//! [`State`] plus the free functions in this module are pure and unit-tested: they take `now` in
//! milliseconds instead of reading a clock. [`start`] spawns one controller thread that owns the
//! live state, samples the pointer at ~30 Hz while the panel is visible, and is the only place
//! that calls [`panel::show`]/[`panel::hide`] or emits the frontend `reveal`/`tuck` events.
//! Hotkeys, tray and the second-instance handler only send [`Command`]s into that thread, so two
//! input paths can never race the window state.
//!
//! # Capture overlay suspension (Task 16 review fix)
//!
//! The own-capture overlay also needs the line hidden while the frozen screen is displayed. It does
//! **not** touch the panel itself: [`suspend_for_capture`] sends a command, the controller hides the
//! line with a capture reason and records the pre-capture visibility plus a pending tuck deadline.
//! While the overlay exists every other reveal path (watcher new item, toggle, top-edge dwell) is
//! suppressed and at most one new-item request is deferred; [`resume_after_capture`] restores the
//! recorded state after the overlay is destroyed and then flushes that deferred/trailing reveal. A
//! watcher item therefore can never pull the topmost panel over the frozen screen.

//!
//! # Reasons
//!
//! `reveal` carries `{ reason: "newItem" | "toggle" | "topEdge" | "secondInstance", peek: bool }`
//! and `tuck` carries `{ reason: "pointerLeave" | "toggle" | "empty" | "dismiss" }` (design spec
//! 4.4 lists the events; Task 11 extends the payload with `peek` for the spring animation).
//! The panel frontend is the only listener, so both events are emitted to the panel webview; the
//! watchers' `reveal-request` is a separate Rust-only event name and never reaches the webview.
//!
//! # Polling while hidden
//!
//! While the panel is hidden the loop normally sleeps for [`HIDDEN_INTERVAL`] without touching the
//! window (design spec 4.6). Two cheap exceptions: the optional top-edge dwell needs a 30 Hz
//! cursor probe, and a click-dismiss suppression has to see the pointer leave the band. Both only
//! read the pointer, never the window; the top-edge test additionally re-enumerates the monitors
//! at most once per second so it can trigger on a display the panel is not currently placed on.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Listener, Manager};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};

use crate::app_state::{AppState, PublicState};
use crate::dragout;
use crate::geometry::{self, dp_to_px, PhysicalRect};
use crate::panel;

/// Visible-panel polling cadence (~30 Hz, design spec 4.6).
pub const POLL_INTERVAL: Duration = Duration::from_millis(33);

/// Hidden-panel idle cadence; no window APIs are touched while hidden.
pub const HIDDEN_INTERVAL: Duration = Duration::from_millis(200);

/// Default peek duration for a new item (`revealPeekMs` default, spec 3.6).
pub const PEEK_MS: u64 = 2500;

/// How long the pointer must stay outside the panel+band before the line tucks.
pub const POINTER_TUCK_MS: u64 = 500;

/// How long an empty, unpinned line stays revealed (spec §3.4 tuck path 2).
pub const EMPTY_HIDE_MS: u64 = 700;

/// How long the pointer must dwell at the screen's top edge for the optional reveal path.
pub const TOP_EDGE_DWELL_MS: u64 = 500;

/// Height of the click-through/pointer band below the panel, in dp (spec §3.4 tuck path 1).
pub const BAND_DP: f64 = 40.0;

/// Tolerance above the work-area top that counts as "the screen's top edge", in dp.
pub const TOP_EDGE_BAND_DP: f64 = 2.0;

/// How often the hidden top-edge poll re-enumerates the monitors, in milliseconds.
///
/// The top-edge test is per monitor (the pointer may be on a display the panel is not placed
/// on), so the hidden loop needs the monitor list; enumerating at the 30 Hz hidden cadence would
/// be wasteful. A display change is picked up within one second; the placement itself
/// re-enumerates the live monitors, so the cache can only delay detection, never misplace the
/// line.
pub const HIDDEN_EDGE_MONITOR_REFRESH_MS: u64 = 1000;

/// Time the frontend tuck animation gets before the window is hidden (spec: `easeIn 220 ms`).
pub const TUCK_ANIMATION_MS: u64 = 300;

/// A tray/menu toggle this soon after a click-dismiss is the same click: keep the line hidden.
pub const TOGGLE_DISMISS_GUARD_MS: u64 = 600;

/// How long a capture request waits for the controller to hide the line before the freeze.
///
/// The controller only does a synchronous `panel::hide` here, so this is a safety bound against a
/// wedged controller thread; a timeout aborts the capture instead of freezing a tainted screen.
pub const CAPTURE_SUSPEND_TIMEOUT: Duration = Duration::from_secs(2);

/// Thread name of the controller loop (shows up in crash/log dumps).
pub const THREAD_NAME: &str = "hangline-reveal";

/// Shared "a menu, press or drag owns the mouse" flag, fed by the frontend and `dragout`.
static BUSY: AtomicBool = AtomicBool::new(false);

/// Shared "the line was explicitly summoned" flag, read by the full-screen poller.
///
/// The poller runs on its own thread but the reveal controller owns [`State::pinned`]; publishing
/// it through this atomic lets the poller evaluate the spec's `pinned && showOverFullScreen`
/// override without a round-trip into the controller channel. The controller refreshes it at the
/// top of every loop iteration, so the poller sees a change within one poll interval.
static PINNED: AtomicBool = AtomicBool::new(false);

/// Shared "an external drag needs the line visible / not tucked" flag (v1.5.0 drag-in).
///
/// Set by the drop-in drag watcher for the duration of an external drag. It pauses the tuck paths
/// (empty-line timeout, pointer leave) without touching the frontend press/menu busy flag.
static DROP_HOLD: AtomicBool = AtomicBool::new(false);

/// Why the line was revealed (frontend `reveal { reason }`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RevealReason {
    /// A watcher hung a new item and `revealOnNew` is on.
    NewItem,
    /// The toggle hotkey, tray item or second launch.
    Toggle,
    /// Optional pointer dwell at the screen's top edge.
    TopEdge,
    /// A second application launch toggled the existing line.
    SecondInstance,
    /// An external image drag reached the screen edge (v1.5.0 drag-in).
    Drop,
}

impl RevealReason {
    /// The wire string used by the `reveal` event.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NewItem => "newItem",
            Self::Toggle => "toggle",
            Self::TopEdge => "topEdge",
            Self::SecondInstance => "secondInstance",
            Self::Drop => "drop",
        }
    }
}

/// Why the line was tucked (frontend `tuck { reason }`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TuckReason {
    /// The pointer stayed outside the panel+band for [`POINTER_TUCK_MS`].
    PointerLeave,
    /// The toggle path hid a pinned/revealed line.
    Toggle,
    /// The line is empty and was revealed longer than [`EMPTY_HIDE_MS`].
    Empty,
    /// A left click outside the panel dismissed the line.
    Dismiss,
    /// A full-screen game/presentation took the screen (design spec §3.2, M4).
    Fullscreen,
}

impl TuckReason {
    /// The wire string used by the `tuck` event.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PointerLeave => "pointerLeave",
            Self::Toggle => "toggle",
            Self::Empty => "empty",
            Self::Dismiss => "dismiss",
            Self::Fullscreen => "fullscreen",
        }
    }
}

/// The reveal/tuck state from design spec §3.4.
///
/// `wanted` is the target visibility, `revealed` the current/animating one. All timestamps are
/// monotonic milliseconds supplied by the caller, so the pure functions are deterministic.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    /// The line should be on screen (shown or being revealed).
    pub wanted: bool,
    /// The line is on screen (or an animation towards/away from the screen is in flight).
    pub revealed: bool,
    /// Explicitly summoned; pointer leave and the empty-line timeout do not tuck it.
    pub pinned: bool,
    /// New-item peek deadline (`None` = no active peek).
    pub peek_until: Option<u64>,
    /// A click-outside hides the line until the pointer leaves the trigger band.
    pub dismissed_until_leave: bool,
    /// First tick at which the pointer was outside the panel+band.
    pub outside_since: Option<u64>,
    /// Time of the last reveal (start of the empty-line 700 ms window).
    pub revealed_at: u64,
    /// First tick at which the pointer rested on the screen's top edge.
    pub top_edge_since: Option<u64>,
    /// A full-screen app currently gates every reveal path (design spec §3.2).
    pub fullscreen_suppressed: bool,
    /// Whether the line was visible/pinned when full-screen suppression started; the exit path
    /// restores exactly that wish (`None` = no suppression has been recorded).
    pub wish_before_suppression: Option<bool>,
    /// New-item reveals that arrived while suppressed (one deferred peek suffices on restore).
    pub fullscreen_deferred_new_items: u32,
}

/// Returns whether a peek is still running at `now`.
fn peek_active(state: &State, now: u64) -> bool {
    state.peek_until.is_some_and(|until| now < until)
}

/// Clears an expired peek deadline (so the state does not carry stale timestamps).
fn expire_peek(state: &mut State, now: u64) {
    if state.peek_until.is_some_and(|until| now >= until) {
        state.peek_until = None;
    }
}

/// Hides the line in memory; the controller emits `tuck` and schedules the window hide.
fn tuck_now(state: &mut State) {
    state.wanted = false;
    state.revealed = false;
    state.pinned = false;
    state.peek_until = None;
    state.outside_since = None;
    state.top_edge_since = None;
}

/// Starts a new-item peek; returns whether the controller should show/animate the panel.
///
/// A new item while the pointer is still inside the band resets the outside timer, so the line
/// stays long enough to be seen. A dismissed line ignores the peek (until the pointer leaves).
pub fn on_new_item(state: &mut State, now: u64) -> bool {
    on_new_item_for(state, now, PEEK_MS)
}

/// [`on_new_item`] with the user-configured `revealPeekMs` (0 ms is allowed and behaves as a
/// normal reveal that tucks on the next pointer-leave tick).
pub fn on_new_item_for(state: &mut State, now: u64, peek_ms: u64) -> bool {
    if state.dismissed_until_leave {
        return false;
    }
    let should_show = !state.revealed;
    state.peek_until = Some(now.saturating_add(peek_ms));
    state.wanted = true;
    state.revealed = true;
    state.revealed_at = now;
    state.outside_since = None;
    state.top_edge_since = None;
    should_show
}

/// Reports a pointer outside the panel+band; returns whether the line should tuck now.
///
/// Call every tick while outside: the first call starts the 500 ms debounce, and the peek or the
/// pinned state can still veto the tuck until it ends.
pub fn on_pointer_leave(state: &mut State, now: u64) -> bool {
    if !state.revealed || state.pinned {
        return false;
    }
    let since = *state.outside_since.get_or_insert(now);
    if peek_active(state, now) {
        return false;
    }
    if now.saturating_sub(since) < POINTER_TUCK_MS {
        return false;
    }
    tuck_now(state);
    true
}

/// A left click landed outside the panel while revealed: tuck immediately and suppress re-reveals.
pub fn on_click_outside(state: &mut State, _now: u64) -> bool {
    if !state.revealed {
        return false;
    }
    tuck_now(state);
    state.dismissed_until_leave = true;
    true
}

/// Whether a tray/menu toggle is the release of the click that just dismissed the line.
///
/// Clicking the tray icon or one of our own menu items is a left click outside the panel, so the
/// controller tucks the line. The tray/menu action then posts its toggle on button release;
/// without this guard that toggle would immediately re-reveal the line the same click just hid.
fn pointer_toggle_was_the_dismiss(state: &State, dismissed_at: Option<u64>, now: u64) -> bool {
    !state.revealed
        && dismissed_at.is_some_and(|at| now.saturating_sub(at) <= TOGGLE_DISMISS_GUARD_MS)
}

/// Explicit toggle (hotkey, tray, second instance): returns `true` to reveal, `false` to tuck.
///
/// An explicit toggle always wins over `dismissed_until_leave` (the user asked for the line) and
/// reveals even an empty line (ruling for Task 11; hover paths still refuse an empty line).
pub fn on_toggle(state: &mut State, now: u64) -> bool {
    if state.revealed {
        tuck_now(state);
        return false;
    }
    state.wanted = true;
    state.revealed = true;
    state.pinned = true;
    state.peek_until = None;
    state.dismissed_until_leave = false;
    state.outside_since = None;
    state.top_edge_since = None;
    state.revealed_at = now;
    true
}

/// The frontend reported its first painted layout and the window is about to be shown.
///
/// This is the startup path inherited from Task 10: the first frame is painted off-screen and
/// then shown. State-wise it behaves like an unpinned reveal without a peek, so the empty-line
/// timeout (700 ms) applies from here.
pub fn on_ready(state: &mut State, now: u64) {
    state.wanted = true;
    state.revealed = true;
    state.pinned = false;
    state.peek_until = None;
    state.outside_since = None;
    state.top_edge_since = None;
    state.revealed_at = now;
}

/// One polling tick for the visible panel; returns the tuck reason when it should hide.
///
/// Busy gestures (menu open, pointer pressed, drag-out) reset the pointer-leave debounce instead
/// of accumulating it: when the gesture ends, a full 500 ms outside window is required.
pub fn tick(
    state: &mut State,
    now: u64,
    item_count: usize,
    pointer_inside_band: bool,
    busy: bool,
) -> Option<TuckReason> {
    expire_peek(state, now);

    if pointer_inside_band {
        state.outside_since = None;
    } else {
        // The pointer has left the trigger band: a click-dismiss no longer suppresses reveals.
        state.dismissed_until_leave = false;
    }

    if !state.revealed {
        return None;
    }

    if pointer_inside_band {
        if !busy
            && item_count == 0
            && !state.pinned
            && !peek_active(state, now)
            && now.saturating_sub(state.revealed_at) >= EMPTY_HIDE_MS
        {
            tuck_now(state);
            return Some(TuckReason::Empty);
        }
        return None;
    }

    if busy || state.pinned {
        state.outside_since = None;
        return None;
    }

    if peek_active(state, now) {
        state.outside_since.get_or_insert(now);
        return None;
    }

    // An empty line disappears 700 ms after the reveal even while the pointer is away.
    if item_count == 0 && now.saturating_sub(state.revealed_at) >= EMPTY_HIDE_MS {
        tuck_now(state);
        return Some(TuckReason::Empty);
    }

    if on_pointer_leave(state, now) {
        Some(TuckReason::PointerLeave)
    } else {
        None
    }
}

/// Optional top-edge dwell (only called when `topEdgeReveal` is on and the line is non-empty).
pub fn on_top_edge(state: &mut State, now: u64, at_edge: bool) -> bool {
    if !at_edge || state.revealed || state.dismissed_until_leave {
        state.top_edge_since = None;
        return false;
    }
    let since = *state.top_edge_since.get_or_insert(now);
    if now.saturating_sub(since) < TOP_EDGE_DWELL_MS {
        return false;
    }
    state.wanted = true;
    state.revealed = true;
    state.pinned = false;
    state.peek_until = None;
    state.outside_since = None;
    state.revealed_at = now;
    state.top_edge_since = None;
    true
}

/// Sets the shared busy flag from the panel frontend (`set_panel_busy`).
pub fn set_busy(busy: bool) {
    BUSY.store(busy, Ordering::Relaxed);
}

/// Returns whether the frontend currently owns a press/menu gesture.
pub fn is_busy() -> bool {
    BUSY.load(Ordering::Relaxed)
}

/// Returns whether the reveal state currently considers the line explicitly summoned.
///
/// Read by the full-screen poller to evaluate the `pinned && showOverFullScreen` override.
pub fn is_pinned() -> bool {
    PINNED.load(Ordering::Relaxed)
}

/// What the controller must restore when full-screen suppression ends (pure, unit-tested).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FullscreenRestore {
    /// The line was visible (or pinned) when the suppression started and must be shown again.
    pub was_wanted: bool,
    /// The line was pinned before the suppression: restore it pinned instead of as a peek.
    pub pinned: bool,
    /// New-item reveals deferred while suppressed; one peek is enough once the line is back.
    pub deferred_new_items: u32,
}

/// Enters full-screen suppression; returns whether the controller must tuck a visible line.
///
/// The pre-suppression wish (visible **or** pinned) is recorded so the exit path can restore it.
/// The state is hidden in memory but `pinned` is preserved: a line that was pinned before the
/// game started comes back pinned, and the poller can still see the pin for the explicit
/// `showOverFullScreen` override. The click-dismiss suppression is cleared because a full-screen
/// app is a context change; otherwise a stale dismiss could block the restore. The visible-reveal
/// timers are cleared **unconditionally**: a stale peek deadline or dwell timestamp left over from
/// a hidden line would otherwise be able to re-show/tuck the line right after the restore.
pub fn on_fullscreen_enter(state: &mut State) -> bool {
    if state.fullscreen_suppressed {
        return false;
    }
    let was_visible = state.revealed || state.wanted;
    let was_pinned = state.pinned;
    state.fullscreen_suppressed = true;
    state.wish_before_suppression = Some(was_visible || was_pinned);
    state.peek_until = None;
    state.outside_since = None;
    state.top_edge_since = None;
    if was_visible {
        state.wanted = false;
        state.revealed = false;
        state.dismissed_until_leave = false;
        // Keep the pinned intent for the restore (and for the poller's override decision).
        state.pinned = was_pinned;
    }
    was_visible
}

/// Records the skipped startup reveal as the full-screen restore wish.
///
/// The READY baseline normally performs the non-pinned startup reveal. When it lands while a
/// full-screen app owns the screen (or the gate flips between `report_panel_ready`'s visibility
/// check and this baseline), the reveal is skipped and this wish remembers it:
/// [`apply_fullscreen_restore`] replays one peek when the app goes away. Without it the line would
/// never appear after a game that was already running at launch.
pub fn on_fullscreen_startup(state: &mut State) {
    state.fullscreen_suppressed = true;
    state.wish_before_suppression = Some(true);
    state.pinned = false;
    state.peek_until = None;
    state.outside_since = None;
    state.top_edge_since = None;
}

/// Ends full-screen suppression and returns the restore plan; a no-op when not suppressed.
pub fn on_fullscreen_exit(state: &mut State) -> FullscreenRestore {
    if !state.fullscreen_suppressed {
        return FullscreenRestore::default();
    }
    state.fullscreen_suppressed = false;
    FullscreenRestore {
        was_wanted: state.wish_before_suppression.take().unwrap_or(false),
        pinned: state.pinned,
        deferred_new_items: std::mem::take(&mut state.fullscreen_deferred_new_items),
    }
}

/// Applies a toggle while full-screen suppression is active.
///
/// The line is hidden, so a toggle expresses "pin the line" instead of show/hide: it sets
/// `pinned = true` and records `wish_before_suppression = Some(true)`, so the line comes back
/// pinned when the full-screen app ends. The toggle is idempotent while suppressed: the line the
/// user would expect to un-pin is not visible, so a second press cannot take the wish back (the
/// restore stays). A visible pinned line is un-pinned by a normal toggle after the restore or the
/// override, exactly like every other toggle. With `showOverFullScreen` also on, the spec's
/// explicit override (`pinned && showOverFullScreen`) opens the gate and returns `true` so the
/// controller shows the line immediately.
pub fn on_suppressed_toggle(state: &mut State, show_over_full_screen: bool) -> bool {
    debug_assert!(
        state.fullscreen_suppressed,
        "on_suppressed_toggle needs an active full-screen suppression"
    );
    state.pinned = true;
    state.wish_before_suppression = Some(true);
    if show_over_full_screen {
        // The user asked for the line and allowed it over full-screen apps: leave the gate. The
        // pending new-item deferral is superseded by the explicit user action.
        state.fullscreen_suppressed = false;
        state.wish_before_suppression = None;
        state.fullscreen_deferred_new_items = 0;
        true
    } else {
        false
    }
}

/// How long the controller waits for the panel frontend's ready handshake (Task 15 READY gating).
///
/// The panel window is created hidden and only becomes visible after `report_panel_ready`; if that
/// never arrives (a broken webview, a frontend crash before the first layout) the tray/hotkey paths
/// must not stay dead forever, so after this timeout the controller unblocks and shows the
/// click-through panel anyway.
pub const READY_TIMEOUT: Duration = Duration::from_secs(3);

/// What a ready/timeout event must do with the commands queued while the panel was not ready.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReadyFlush {
    /// Whether the normal startup reveal ([`on_ready`]) must run (queue was empty).
    pub baseline: bool,
    /// Number of queued toggles to replay in arrival order.
    pub toggles: u32,
    /// Number of queued new-item reveals to replay in arrival order.
    pub new_items: u32,
    /// True when the 3 s fallback fired instead of the real handshake.
    pub fallback: bool,
    /// True when the event is a duplicate (the queue was already flushed).
    pub already_ready: bool,
}

/// READY gating state machine (Task 15; pure and unit-tested).
///
/// Before the panel frontend reports ready, any tray/hotkey/second-instance toggle or new-item
/// reveal is *queued*: a window that has no painted frame must not be shown/tucked by the controller
/// yet, and the request must not be lost either. On the handshake the queued commands are replayed
/// in arrival order; `on_ready`'s startup reveal runs only when nothing was queued, so it cannot
/// clobber the user's earlier action (spec carry-over: "on_ready 不得 clobber 排队状态").
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReadyQueue {
    ready: bool,
    toggles: u32,
    new_items: u32,
}

impl ReadyQueue {
    /// Whether the queue has been flushed (real handshake or timeout fallback).
    #[cfg(test)]
    pub fn is_ready(&self) -> bool {
        self.ready
    }

    /// Number of queued toggles (test/diagnostics helper).
    #[cfg(test)]
    pub fn queued_toggles(&self) -> u32 {
        self.toggles
    }

    /// Number of queued new-item reveals (test/diagnostics helper).
    #[cfg(test)]
    pub fn queued_new_items(&self) -> u32 {
        self.new_items
    }

    /// Records a toggle; returns `true` when it must be queued instead of applied now.
    pub fn on_toggle(&mut self) -> bool {
        if self.ready {
            return false;
        }
        self.toggles = self.toggles.saturating_add(1);
        true
    }

    /// Records a new-item reveal; returns `true` when it must be queued instead of applied now.
    pub fn on_new_item(&mut self) -> bool {
        if self.ready {
            return false;
        }
        self.new_items = self.new_items.saturating_add(1);
        true
    }

    /// Handles the real ready handshake; returns what to replay (idempotent).
    pub fn on_ready(&mut self) -> ReadyFlush {
        self.flush(false)
    }

    /// Handles the 3 s fallback timeout; returns what to replay (idempotent).
    pub fn on_timeout(&mut self) -> ReadyFlush {
        self.flush(true)
    }

    /// Shared flush logic for the handshake and the timeout.
    fn flush(&mut self, fallback: bool) -> ReadyFlush {
        if self.ready {
            return ReadyFlush {
                already_ready: true,
                fallback,
                ..ReadyFlush::default()
            };
        }
        self.ready = true;
        let flush = ReadyFlush {
            baseline: self.toggles == 0 && self.new_items == 0,
            toggles: self.toggles,
            new_items: self.new_items,
            fallback,
            already_ready: false,
        };
        self.toggles = 0;
        self.new_items = 0;
        flush
    }
}

/// The pre-capture line state the controller snapshots when it suspends for the overlay.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CaptureSuspension {
    /// The line was visible when the capture started (it must be shown again on resume).
    was_visible: bool,
    /// A tuck was already scheduled; restoring the deadline keeps the line from staying up forever.
    hide_deadline: Option<u64>,
}

/// What a suspended pending tuck deadline means at resume time (pure and unit-tested).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ResumeHide {
    /// No pending tuck (or the line was hidden): nothing to do.
    None,
    /// The deadline is still in the future: restore it so the line tucks when it expires.
    Pending(u64),
    /// The deadline elapsed while the overlay was open: leave the restored line hidden.
    HideNow,
}

/// Decides what to do with a suspended pending tuck deadline on resume.
fn resume_hide_plan(was_visible: bool, deadline: Option<u64>, now: u64) -> ResumeHide {
    match (was_visible, deadline) {
        (true, Some(deadline)) if deadline <= now => ResumeHide::HideNow,
        (true, Some(deadline)) => ResumeHide::Pending(deadline),
        _ => ResumeHide::None,
    }
}

/// The restore decision of [`CaptureGate::resume`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CaptureResume {
    /// What the line looked like before the overlay took it over.
    suspension: CaptureSuspension,
    /// A watcher/capture new-item arrived while the overlay was open and must be flushed on resume.
    queued_new_item: bool,
}

/// Capture-overlay suspension gate (pure; unit-tested).
///
/// While an overlay exists the controller must not let any other path show or tuck the line: a
/// watcher new-item would put the topmost panel over the frozen screen and intercept the selection.
/// The gate records the suspension and defers at most one new-item request until the resume.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct CaptureGate {
    suspension: Option<CaptureSuspension>,
    queued_new_item: bool,
}

impl CaptureGate {
    /// Starts the suspension and returns the pre-capture state the controller must restore.
    fn suspend(&mut self, was_visible: bool, hide_deadline: Option<u64>) -> CaptureSuspension {
        let suspension = CaptureSuspension {
            was_visible,
            hide_deadline,
        };
        self.suspension = Some(suspension);
        self.queued_new_item = false;
        suspension
    }

    /// Whether an overlay currently owns the screen.
    fn active(&self) -> bool {
        self.suspension.is_some()
    }

    /// Defers a new-item reveal while suspended; `false` when no capture is active.
    fn defer_new_item(&mut self) -> bool {
        if !self.active() {
            return false;
        }
        self.queued_new_item = true;
        true
    }

    /// Ends the suspension exactly once; `None` when no capture was suspended.
    fn resume(&mut self) -> Option<CaptureResume> {
        let suspension = self.suspension.take()?;
        Some(CaptureResume {
            suspension,
            queued_new_item: std::mem::take(&mut self.queued_new_item),
        })
    }
}

/// The full-screen half of a capture resume (pure; unit-tested).
///
/// The capture overlay is one of Hangline's own windows, so [`crate::fullscreen`]'s foreground
/// probe excludes it: while the overlay covered the screen the poller could only observe "normal"
/// and may have already told the controller to leave the gate (or never told it to enter even
/// though a game was behind the overlay). The controller therefore re-probes after the overlay is
/// gone and this plan decides what to do with the *recorded* gate state:
///
/// - both normal: run the normal capture resume;
/// - the probe still sees a full-screen app: stay hidden, entering the gate if the recorded state
///   had already left it;
/// - the recorded gate is active but the app is gone: exit now and restore the wish (instead of
///   waiting for a poller that already applied "normal").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CaptureFullscreenPlan {
    /// No full-screen app before or after the overlay: run the normal resume.
    Normal,
    /// A full-screen app still owns the screen: keep the line hidden and preserve the wish.
    StaySuppressed {
        /// The recorded state says normal, so the controller must enter the gate itself.
        enter: bool,
    },
    /// The gate is recorded but the app is gone: exit it and restore the wish now.
    Restore,
}

/// Decides the full-screen half of a capture resume from the recorded and freshly probed gates.
fn capture_fullscreen_plan(
    recorded_suppressed: bool,
    probed_suppressed: bool,
) -> CaptureFullscreenPlan {
    match (recorded_suppressed, probed_suppressed) {
        (false, false) => CaptureFullscreenPlan::Normal,
        (false, true) => CaptureFullscreenPlan::StaySuppressed { enter: true },
        (true, true) => CaptureFullscreenPlan::StaySuppressed { enter: false },
        (true, false) => CaptureFullscreenPlan::Restore,
    }
}

/// How often the controller re-probes a full-screen gate it entered from its own capture-resume
/// probe (not from the poller).
///
/// The poller remains the long-term owner of the gate and normally re-confirms a real full-screen
/// app within one debounce window ([`crate::fullscreen::STABLE_MS`] plus one poll). Until it does,
/// this fallback guarantees no stuck suppression: if the app is already gone the controller exits
/// and restores on its own, and if it is still there the probe keeps the line hidden and the
/// poller's eventual exit drives the restore.
pub const FULLSCREEN_RECHECK_MS: u64 = 1000;

/// What the controller-side full-screen recheck must do (pure; unit-tested).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FullscreenRecheck {
    /// The poller re-confirmed the gate: it now owns the eventual exit.
    Confirmed,
    /// The app is gone: exit the gate and restore the wish without waiting for a poller event
    /// (the poller may already have applied "normal" while the capture overlay hid the app).
    Restore,
    /// The probe still sees the app but the poller has not confirmed it yet: probe again later.
    Retry,
}

/// Decides one controller-side full-screen recheck.
fn fullscreen_recheck(probed_suppressed: bool, poller_suppressed: bool) -> FullscreenRecheck {
    match (probed_suppressed, poller_suppressed) {
        (false, _) => FullscreenRecheck::Restore,
        (true, true) => FullscreenRecheck::Confirmed,
        (true, false) => FullscreenRecheck::Retry,
    }
}

/// One command the controller held back until the panel reported ready.
enum PendingAction {
    /// A toggle (tray/hotkey/second launch) in arrival order.
    Toggle {
        reason: RevealReason,
        from_pointer: bool,
    },
    /// A watcher's new-item reveal in arrival order.
    NewItem,
}

/// Commands accepted by the controller thread.
enum Command {
    /// A watcher hung a new item (Rust-only `reveal-request` event with `reason: "newItem"`).
    NewItem,
    /// The user asked for a toggle through a hotkey/tray/second launch.
    Toggle {
        reason: RevealReason,
        /// `true` for a tray icon/menu click (the click itself may already have dismissed the
        /// line; see [`pointer_toggle_was_the_dismiss`]).
        from_pointer: bool,
    },
    /// The panel frontend finished its first layout (Task 10 ready handshake).
    Ready,
    /// [`READY_TIMEOUT`] elapsed without the handshake (Task 15 fallback).
    ReadyTimeout,
    /// A capture overlay is about to freeze the screen: hide the line now and remember what to
    /// restore. The reply carries whether the line was visible before the suspension.
    SuspendForCapture { reply: Sender<bool> },
    /// The capture overlay is gone: restore the pre-capture visibility/deadline, then flush a
    /// deferred (or the capture's own) new-item reveal. `request_new_item` is set when the capture
    /// hung a new item and should peek exactly like a watcher item.
    ResumeAfterCapture { request_new_item: bool },
    /// The full-screen poller observed a gate change (design spec §3.2, M4).
    FullscreenSuppressed { suppressed: bool },
    /// An external image drag reached the screen's top edge: show the line for the drop (v1.5.0).
    DropReveal,
    /// The external drag ended: the drop hold flag is cleared by the watcher (v1.5.0).
    DropEnd,
}

/// Handle to the single reveal controller; managed by Tauri so every input path can post to it.
pub struct RevealController {
    tx: Sender<Command>,
}

/// Starts the controller thread and registers the Rust-only new-item request listener.
///
/// The listener deliberately consumes the watchers' `reveal-request` before any webview: watchers
/// emit it to [`EventTarget::App`], the controller re-emits the frontend contract with the final
/// `reason`/`peek` decision. Registering this before the watchers start is required by `lib.rs`.
pub fn start(app: &AppHandle) -> RevealController {
    let (tx, rx) = mpsc::channel();
    let request_tx = tx.clone();
    app.listen("reveal-request", move |_event| {
        if let Err(err) = request_tx.send(Command::NewItem) {
            log::warn!("reveal: controller channel is closed; dropping a new-item request ({err})");
        }
    });

    let thread_app = app.clone();
    if let Err(err) = thread::Builder::new()
        .name(THREAD_NAME.to_string())
        .spawn(move || run(&thread_app, rx))
    {
        log::error!("reveal: failed to spawn the controller thread: {err}");
    }
    RevealController { tx }
}

/// Posts a toggle to the controller (hotkey, second instance, or a tray menu action).
pub fn toggle(app: &AppHandle, reason: RevealReason) {
    post_toggle(app, reason, false);
}

/// Posts a toggle produced by a tray icon/menu click (guards against double-handling the click).
pub fn toggle_from_pointer(app: &AppHandle, reason: RevealReason) {
    post_toggle(app, reason, true);
}

fn post_toggle(app: &AppHandle, reason: RevealReason, from_pointer: bool) {
    if let Some(controller) = app.try_state::<RevealController>() {
        if let Err(err) = controller.tx.send(Command::Toggle {
            reason,
            from_pointer,
        }) {
            log::warn!("reveal: controller channel is closed; cannot toggle ({err})");
        }
    } else {
        // Startup race (before `start`) or a unit-test harness: fall back to the raw window
        // toggle so the hotkey is never silently dead.
        log::warn!("reveal: controller not managed yet; falling back to the raw panel toggle");
        panel::toggle(app);
    }
}

/// Asks the controller to show the line for an in-flight external drag (v1.5.0 drag-in).
///
/// Unlike a toggle this never hides anything: it only reveals a hidden line, without pinning it.
pub fn request_drop_reveal(app: &AppHandle) {
    if let Some(controller) = app.try_state::<RevealController>() {
        if let Err(err) = controller.tx.send(Command::DropReveal) {
            log::warn!("reveal: controller channel is closed; cannot reveal for a drop ({err})");
        }
    }
}

/// Ends a drop reveal: the watcher clears this on mouse-up. The next tick re-evaluates the tuck.
pub fn end_drop_reveal(app: &AppHandle) {
    if let Some(controller) = app.try_state::<RevealController>() {
        if let Err(err) = controller.tx.send(Command::DropEnd) {
            log::warn!("reveal: controller channel is closed; cannot end a drop reveal ({err})");
        }
    }
}

/// Sets the "external drag in progress" hold that pauses tuck decisions (v1.5.0 drag-in).
pub fn set_drop_hold(held: bool) {
    DROP_HOLD.store(held, Ordering::Relaxed);
}

/// Returns whether an external drag currently holds the line open.
pub fn drop_is_held() -> bool {
    DROP_HOLD.load(Ordering::Relaxed)
}

/// Called by `report_panel_ready` after the first frame has been painted.
pub fn on_ready_request(app: &AppHandle) {
    if let Some(controller) = app.try_state::<RevealController>() {
        if let Err(err) = controller.tx.send(Command::Ready) {
            log::warn!("reveal: controller channel is closed; cannot report ready ({err})");
        }
    }
}

/// Called by the 3 s fallback thread in `setup` when the frontend never reports ready.
///
/// The controller handles it exactly once (a real handshake arriving later is a logged no-op):
/// it unblocks the queued requests and shows the click-through panel so the tray/hotkey paths
/// still work. A missing controller (startup failed) is a logged no-op.
pub fn on_ready_timeout(app: &AppHandle) {
    if let Some(controller) = app.try_state::<RevealController>() {
        if let Err(err) = controller.tx.send(Command::ReadyTimeout) {
            log::warn!("reveal: controller channel is closed; cannot report the READY timeout ({err})");
        }
    } else {
        log::warn!("reveal: READY timeout fired before the controller was managed");
    }
}

/// Suspends the line for a capture overlay and waits until the controller applied the hide.
///
/// Called from the capture worker thread (never from the event-loop thread). The controller remains
/// the only owner of `panel::show`/`hide`; it hides the line, records the pre-capture visibility and
/// a pending tuck deadline, and suppresses every other reveal path until
/// [`resume_after_capture`]. Returns whether the line was visible before the suspension. A timeout
/// is an error: the capture must abort rather than freeze a screen that may still show the line.
pub fn suspend_for_capture(app: &AppHandle) -> anyhow::Result<bool> {
    let Some(controller) = app.try_state::<RevealController>() else {
        anyhow::bail!("reveal: the reveal controller is not running; cannot suspend for a capture");
    };
    let (reply, wait) = mpsc::channel();
    controller
        .tx
        .send(Command::SuspendForCapture { reply })
        .map_err(|error| anyhow::anyhow!("reveal: the controller channel is closed: {error}"))?;
    wait.recv_timeout(CAPTURE_SUSPEND_TIMEOUT).map_err(|error| {
        anyhow::anyhow!(
            "reveal: the controller did not suspend the line within {} ms: {error}",
            CAPTURE_SUSPEND_TIMEOUT.as_millis()
        )
    })
}

/// Resumes the reveal controller after the overlay was destroyed (fire-and-forget).
///
/// The controller restores the pre-capture visibility and pending tuck deadline, then runs the
/// new-item peek path for a capture item (`request_new_item`) or for a watcher item that arrived
/// while the overlay was open. The line is never shown while the overlay still exists.
pub fn resume_after_capture(app: &AppHandle, request_new_item: bool) {
    let Some(controller) = app.try_state::<RevealController>() else {
        log::warn!("reveal: controller is not running; cannot resume after a capture");
        return;
    };
    if let Err(error) = controller
        .tx
        .send(Command::ResumeAfterCapture { request_new_item })
    {
        log::warn!("reveal: controller channel is closed; cannot resume after a capture ({error})");
    }
}

/// Posts a full-screen gate change to the controller (called by [`crate::fullscreen`]'s poller).
///
/// The controller is the only owner of the reveal state and the panel window, so the poller never
/// shows/hides anything itself. When the controller is missing (startup race or a unit-test
/// harness) the change is dropped: the poller keeps retrying only on the next gate flip, so this
/// must be logged loudly; the app then behaves as "no suppression", never as "stuck hidden".
pub fn set_fullscreen_suppressed(app: &AppHandle, suppressed: bool) {
    let Some(controller) = app.try_state::<RevealController>() else {
        log::warn!(
            "reveal: controller is not running; dropping full-screen suppression={suppressed}"
        );
        return;
    };
    if let Err(error) = controller
        .tx
        .send(Command::FullscreenSuppressed { suppressed })
    {
        log::warn!(
            "reveal: controller channel is closed; cannot apply full-screen suppression \
             (suppressed={suppressed}, {error})"
        );
    }
}

/// Milliseconds since the controller started; explicit in every log line for timing evidence.
fn millis(origin: Instant) -> u64 {
    u64::try_from(origin.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// Physical panel rectangle plus the downward-extended pointer band.
#[derive(Clone, Copy, Debug)]
struct PanelBounds {
    panel: PhysicalRect,
    band: PhysicalRect,
}

impl PanelBounds {
    /// Extends `panel` downwards by [`BAND_DP`] logical pixels.
    fn new(panel: PhysicalRect, scale: f64) -> Self {
        let scale = if scale.is_finite() && scale > 0.0 {
            scale
        } else {
            1.0
        };
        let band = PhysicalRect {
            x: panel.x,
            y: panel.y,
            w: panel.w,
            h: panel.h.saturating_add(dp_to_px(BAND_DP, scale)),
        };
        Self { panel, band }
    }
}

/// Reads the panel's current physical rectangle; `None` when the window is gone or querying fails.
fn read_bounds(app: &AppHandle) -> Option<PanelBounds> {
    let win = app.get_webview_window(panel::LABEL)?;
    let position = win.outer_position().ok()?;
    let size = win.outer_size().ok()?;
    let scale = win.scale_factor().unwrap_or(1.0);
    Some(PanelBounds::new(
        PhysicalRect {
            x: position.x,
            y: position.y,
            w: i32::try_from(size.width).unwrap_or(i32::MAX),
            h: i32::try_from(size.height).unwrap_or(i32::MAX),
        },
        scale,
    ))
}

/// `GetAsyncKeyState` left-button edge sampling (the OS reports the physical state globally).
fn left_button_down() -> bool {
    // SAFETY: GetAsyncKeyState is a read-only query with no pointers.
    unsafe { (GetAsyncKeyState(VK_LBUTTON.0 as i32) as u16 & 0x8000) != 0 }
}

/// Serialized payload of the frontend `reveal` event.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RevealPayload {
    reason: &'static str,
    peek: bool,
}

/// Serialized payload of the frontend `tuck` event.
#[derive(Clone, Debug, Serialize)]
struct TuckPayload {
    reason: &'static str,
}

/// Emits an event to the panel webview only (never back into the Rust request listener).
fn emit_panel(app: &AppHandle, event: &str, payload: impl Serialize + Clone) {
    if let Err(err) = app.emit_to(panel::LABEL, event, payload) {
        log::warn!("reveal: failed to emit the {event} event: {err}");
    }
}

/// Shows the panel, then tells the frontend to spring in.
fn show_and_emit(app: &AppHandle, reason: RevealReason, peek: bool, now: u64, item_count: usize) {
    panel::show(app);
    emit_panel(
        app,
        "reveal",
        RevealPayload {
            reason: reason.as_str(),
            peek,
        },
    );
    log::info!(
        "reveal: shown (reason {:?}, peek={peek}, items={item_count}) at +{now} ms",
        reason
    );
}

/// Tells the frontend to slide up; the caller schedules the window hide.
fn tuck_and_emit(app: &AppHandle, reason: TuckReason, now: u64, busy: bool, outside_ms: Option<u64>) {
    emit_panel(
        app,
        "tuck",
        TuckPayload {
            reason: reason.as_str(),
        },
    );
    log::info!(
        "reveal: tuck ({:?}) at +{now} ms (busy={busy}, pointer outside {outside_ms:?} ms); \
         hiding in {TUCK_ANIMATION_MS} ms",
        reason
    );
}

/// Applies one explicit toggle to the state machine and drives the window.
///
/// Shared by the live command path and the READY replay, so a queued toggle behaves exactly like
/// one that arrived after the handshake (including the tray/menu click-dismiss guard).
fn apply_toggle(
    app: &AppHandle,
    state: &mut State,
    hide_deadline: &mut Option<u64>,
    reason: RevealReason,
    from_pointer: bool,
    now: u64,
    last_dismiss: Option<u64>,
    left_down: &mut bool,
) {
    let snapshot = snapshot(app);
    let count = snapshot.as_ref().map_or(0, |snapshot| snapshot.items.len());
    let show_over_full_screen = snapshot
        .as_ref()
        .is_some_and(|snapshot| snapshot.settings.show_over_full_screen);
    if state.fullscreen_suppressed {
        // A full-screen app owns the screen: the toggle flips the pin intent and only shows when
        // the spec's explicit override (pinned && showOverFullScreen) allows it.
        if on_suppressed_toggle(state, show_over_full_screen) {
            let revealed = on_toggle(state, now);
            debug_assert!(revealed, "a suppressed line is always hidden before the override");
            *hide_deadline = None;
            show_and_emit(app, reason, false, now, count);
            *left_down = left_button_down();
            log::info!(
                "reveal: full-screen override by a {:?} toggle (pinned && showOverFullScreen); \
                 showing the line (+{now} ms)",
                reason
            );
        } else {
            log::info!(
                "reveal: {:?} toggle while full-screen suppressed (pinned={}, \
                 showOverFullScreen={show_over_full_screen}); staying hidden and keeping the pin \
                 intent for the restore (+{now} ms)",
                reason,
                state.pinned
            );
        }
        return;
    }
    if from_pointer && pointer_toggle_was_the_dismiss(state, last_dismiss, now) {
        log::info!(
            "reveal: tray/menu toggle within {TOGGLE_DISMISS_GUARD_MS} ms of the click-dismiss; \
             keeping the line hidden (at +{now} ms)"
        );
    } else if on_toggle(state, now) {
        *hide_deadline = None;
        show_and_emit(app, reason, false, now, count);
        *left_down = left_button_down();
    } else {
        tuck_and_emit(app, TuckReason::Toggle, now, false, None);
        *hide_deadline = Some(now.saturating_add(TUCK_ANIMATION_MS));
    }
}

/// Flushes the READY queue: the startup reveal (empty queue) or a replay of the queued commands.
///
/// `on_ready`'s baseline reveal runs only when nothing was queued, so it can never clobber a
/// toggle/reveal the user requested earlier (the carry-over contract). Replays keep arrival order.
fn flush_ready(
    app: &AppHandle,
    state: &mut State,
    hide_deadline: &mut Option<u64>,
    left_down: &mut bool,
    last_dismiss: Option<u64>,
    pending: &mut Vec<PendingAction>,
    flush: &ReadyFlush,
    now: u64,
) {
    if flush.already_ready {
        log::info!("reveal: duplicate ready/timeout event ignored at +{now} ms");
        return;
    }
    if flush.baseline {
        if crate::fullscreen::is_suppressed() {
            // The app was started while a full-screen app owns the screen (or the poller won the
            // READY race): keep the freshly painted window hidden and remember the skipped
            // startup reveal as the restore wish, so the line appears after the app is gone.
            on_fullscreen_startup(state);
            if panel::is_visible() {
                // `report_panel_ready` checked the gate *before* sending this command; if the gate
                // flipped in between, the window is already visible. Hide it now instead of
                // letting the poller's tuck animation leave it on screen for another frame.
                panel::hide(app);
            }
            log::info!(
                "reveal: startup reveal skipped while full-screen suppression is active; the line \
                 stays hidden and the startup wish is kept for the restore (+{now} ms)"
            );
            return;
        }
        on_ready(state, now);
        // Idempotent: `report_panel_ready` normally showed the window before sending this command;
        // showing here too closes the reverse race (the gate cleared after its check but before
        // this baseline), where nobody else would show the panel.
        panel::show(app);
        *left_down = left_button_down();
        log::info!(
            "reveal: panel ready at +{now} ms (fallback={}); startup show is a non-pinned reveal \
             (empty auto-hide {EMPTY_HIDE_MS} ms)",
            flush.fallback
        );
        return;
    }
    let queued = std::mem::take(pending);
    log::info!(
        "reveal: flushing {} queued command(s) at +{now} ms after the {} ({} toggle(s), {} new \
         item(s)); the startup reveal is skipped so it cannot clobber them",
        queued.len(),
        if flush.fallback { "READY timeout fallback" } else { "ready handshake" },
        flush.toggles,
        flush.new_items
    );
    for action in queued {
        match action {
            PendingAction::Toggle {
                reason,
                from_pointer,
            } => apply_toggle(
                app,
                state,
                hide_deadline,
                reason,
                from_pointer,
                now,
                last_dismiss,
                left_down,
            ),
            PendingAction::NewItem => {
                if let Some(snapshot) = snapshot(app) {
                    handle_new_item(app, state, hide_deadline, &snapshot, now);
                    *left_down = left_button_down();
                }
            }
        }
    }
}

/// Applies a capture-resume command: restores the suspended visibility/deadline, then flushes.
///
/// Ordering is the point: the overlay is already destroyed when this runs, so showing the line (or
/// running a new-item peek) cannot put the topmost panel over the frozen screen. Before anything is
/// shown the full-screen gate is re-probed ([`crate::fullscreen::probe_state`]): the overlay is
/// own-process and therefore invisible to the poller, so the recorded gate may be stale in either
/// direction while a game was behind the overlay.
fn apply_capture_resume(
    app: &AppHandle,
    state: &mut State,
    hide_deadline: &mut Option<u64>,
    gate: &mut CaptureGate,
    request_new_item: bool,
    now: u64,
    left_down: &mut bool,
    fullscreen_recheck_at: &mut Option<u64>,
) {
    let Some(resume) = gate.resume() else {
        log::info!("reveal: capture resume ignored (no capture suspension was active) at +{now} ms");
        return;
    };
    let CaptureResume {
        suspension,
        queued_new_item,
    } = resume;
    log::info!(
        "reveal: capture ended at +{now} ms (was_visible={}, pending hide deadline {:?}, \
         deferred_new_item={queued_new_item}, request_new_item={request_new_item})",
        suspension.was_visible,
        suspension.hide_deadline
    );

    let probed = crate::fullscreen::probe_state(app);
    match capture_fullscreen_plan(state.fullscreen_suppressed, probed.is_suppressed()) {
        CaptureFullscreenPlan::StaySuppressed { enter } => {
            if enter {
                // The poller applied "normal" while the overlay hid a still-full-screen app (or a
                // game started during the overlay and the poller has not applied it yet). Enter
                // the gate now; the scheduled recheck bounds the case where this probe was a
                // one-tick race, and the poller takes over again as soon as it observes the app.
                on_fullscreen_enter(state);
                *fullscreen_recheck_at = Some(now.saturating_add(FULLSCREEN_RECHECK_MS));
                log::info!(
                    "reveal: capture ended over a full-screen app the poller could not see; \
                     re-entering full-screen suppression (+{now} ms, probed reason {:?})",
                    probed.reason()
                );
            }
            // Fold the pre-capture visibility into the restore wish so the line comes back after
            // *both* gates are gone, and keep a queued new item deferred instead of peeking over
            // the game.
            if suspension.was_visible {
                state.wish_before_suppression = Some(true);
            }
            if queued_new_item || request_new_item {
                if let Some(snapshot) = snapshot(app) {
                    handle_new_item(app, state, hide_deadline, &snapshot, now);
                }
            }
            *left_down = left_button_down();
            log::info!(
                "reveal: capture ended while full-screen suppression is active; keeping the line \
                 hidden (+{now} ms, wish={:?})",
                state.wish_before_suppression
            );
            return;
        }
        CaptureFullscreenPlan::Restore => {
            // The recorded gate was active but the app is gone (it may have exited while the
            // overlay was up, in which case the poller already applied "normal"). Exit now and
            // restore the wish instead of waiting for a poller event that will never come.
            let mut restore = on_fullscreen_exit(state);
            restore.was_wanted |= suspension.was_visible;
            if queued_new_item || request_new_item {
                restore.deferred_new_items = restore.deferred_new_items.saturating_add(1);
            }
            *fullscreen_recheck_at = None;
            log::info!(
                "reveal: capture ended after the full-screen app went away; restoring now \
                 (+{now} ms, wish={}, pinned={}, deferred={})",
                restore.was_wanted,
                restore.pinned,
                restore.deferred_new_items
            );
            apply_fullscreen_restore(app, state, hide_deadline, restore, now, left_down);
            return;
        }
        CaptureFullscreenPlan::Normal => {}
    }

    match resume_hide_plan(suspension.was_visible, suspension.hide_deadline, now) {
        ResumeHide::None => {
            if suspension.was_visible {
                panel::show(app);
                log::info!("reveal: restored the line (it was visible before the capture)");
            }
        }
        ResumeHide::Pending(deadline) => {
            panel::show(app);
            *hide_deadline = Some(deadline);
            log::info!(
                "reveal: restored the line and its pending tuck deadline (+{} ms remaining)",
                deadline.saturating_sub(now)
            );
        }
        ResumeHide::HideNow => {
            // The pending tuck expired during the overlay: keep the window hidden and let the next
            // visible tick finish the state machine (the line must not flash back for a frame).
            log::info!(
                "reveal: the pending tuck deadline elapsed during the capture; keeping the line \
                 hidden (+{now} ms)"
            );
        }
    }
    *left_down = left_button_down();

    if queued_new_item || request_new_item {
        if let Some(snapshot) = snapshot(app) {
            handle_new_item(app, state, hide_deadline, &snapshot, now);
        }
    }
}

/// Applies one full-screen gate change command (design spec §3.2, M4).
///
/// Enter: records the pre-suppression wish and tucks a visible line (same 300 ms animation as every
/// other tuck). Exit: restores the wish, or the deferred new-item peek. While a capture overlay is
/// active the controller keeps the gate recorded instead of consuming the wish: the overlay is
/// own-process and invisible to the full-screen probe, so a poller "normal" during the overlay
/// cannot be trusted — the capture resume re-probes and either restores or stays suppressed (see
/// [`apply_capture_resume`]). Both directions are idempotent, so a duplicated poll command is a
/// logged no-op.
fn apply_fullscreen_state(
    app: &AppHandle,
    state: &mut State,
    hide_deadline: &mut Option<u64>,
    capture_active: bool,
    suppressed: bool,
    now: u64,
    left_down: &mut bool,
) {
    if suppressed {
        let was_visible = on_fullscreen_enter(state);
        let pinned = state.pinned;
        if was_visible && !capture_active {
            tuck_and_emit(app, TuckReason::Fullscreen, now, is_busy(), None);
            *hide_deadline = Some(now.saturating_add(TUCK_ANIMATION_MS));
        }
        log::info!(
            "reveal: full-screen suppression on at +{now} ms (was_visible={was_visible}, \
             pinned={pinned}, wish={:?}, capture_overlay={capture_active})",
            state.wish_before_suppression
        );
        return;
    }

    if !state.fullscreen_suppressed {
        log::debug!(
            "reveal: full-screen suppression off ignored (already normal) at +{now} ms"
        );
        return;
    }
    if capture_active {
        // Keep the recorded gate (and its wish/deferred items) across the overlay; the capture
        // resume re-probes and applies [`capture_fullscreen_plan`]. Consuming the wish here would
        // lose it if the app is still full-screen when the overlay closes.
        log::info!(
            "reveal: full-screen suppression off while the capture overlay still owns the screen; \
             keeping the gate recorded for the capture-resume re-probe (+{now} ms)"
        );
        return;
    }
    let restore = on_fullscreen_exit(state);
    log::info!(
        "reveal: full-screen suppression off at +{now} ms (wish={}, pinned={}, deferred={})",
        restore.was_wanted,
        restore.pinned,
        restore.deferred_new_items
    );
    apply_fullscreen_restore(app, state, hide_deadline, restore, now, left_down);
}

/// Re-shows the line after full-screen suppression ended, honouring the recorded wish.
///
/// A line that was pinned comes back pinned (exactly like a normal toggle reveal). A peek or an
/// empty line gets a fresh peek so the pre-suppression `peek_until` cannot make it disappear on the
/// next tick; a new item that arrived while suppressed produces the same peek when `revealOnNew`
/// is on (the ReadyQueue analogue: the newest items are shown, one peek is enough).
fn apply_fullscreen_restore(
    app: &AppHandle,
    state: &mut State,
    hide_deadline: &mut Option<u64>,
    restore: FullscreenRestore,
    now: u64,
    left_down: &mut bool,
) {
    let Some(snapshot) = snapshot(app) else {
        return;
    };
    let deferred_peek = restore.deferred_new_items > 0
        && snapshot.settings.reveal_on_new
        && !snapshot.items.is_empty();
    if !restore.was_wanted && !deferred_peek {
        log::info!(
            "reveal: full-screen ended with no restore wish ({} deferred item(s)); the line stays \
             hidden (+{now} ms)",
            restore.deferred_new_items
        );
        return;
    }

    if restore.pinned && restore.was_wanted {
        let revealed = on_toggle(state, now);
        debug_assert!(revealed, "a suppressed line is always hidden before the restore");
        *hide_deadline = None;
        show_and_emit(app, RevealReason::Toggle, false, now, snapshot.items.len());
        *left_down = left_button_down();
        log::info!(
            "reveal: restored the pinned line after full-screen (+{now} ms, {} item(s))",
            snapshot.items.len()
        );
        return;
    }

    let peek_ms = u64::from(snapshot.settings.reveal_peek_ms);
    let should_show = on_new_item_for(state, now, peek_ms);
    if !state.revealed {
        log::info!(
            "reveal: full-screen restore peek suppressed by a click-dismiss; the line stays hidden \
             (+{now} ms)"
        );
        return;
    }
    *hide_deadline = None;
    let reason = if deferred_peek {
        RevealReason::NewItem
    } else {
        RevealReason::Toggle
    };
    if should_show {
        show_and_emit(app, reason, true, now, snapshot.items.len());
    }
    *left_down = left_button_down();
    log::info!(
        "reveal: restored the line after full-screen (+{now} ms, peek={peek_ms} ms, \
         deferred={})",
        restore.deferred_new_items
    );
}

/// The controller loop: one thread owns the state, the timer and every window transition.
fn run(app: &AppHandle, rx: Receiver<Command>) {
    let origin = Instant::now();
    let mut state = State::default();
    let mut hide_deadline: Option<u64> = None;
    let mut bounds: Option<PanelBounds> = None;
    // Cached monitor list for the hidden top-edge test (refreshed once a second, not at 30 Hz).
    let mut edge_monitors: Option<(Vec<geometry::MonitorInfo>, u64)> = None;
    let mut left_down = left_button_down();
    let mut last_dismiss: Option<u64> = None;
    // Task 15 READY gating: queue toggles/new items until the frontend handshake (or the timeout).
    let mut ready = ReadyQueue::default();
    let mut pending: Vec<PendingAction> = Vec::new();
    // Task 14 monitor follow: pure debounce state plus the last follow-poll time.
    let mut follow = panel::MonitorFollow::default();
    let mut last_monitor_poll: Option<u64> = None;
    let mut was_visible = false;
    // Whether the busy-paused follow was already logged for the current gesture.
    let mut follow_paused = false;
    // Task 16 review fix: the capture overlay owns the screen while active; every other reveal
    // path is suppressed and at most one new-item is deferred until the resume.
    let mut capture = CaptureGate::default();
    // A full-screen gate the controller entered from its own capture-resume probe (rather than
    // from a poller command): re-probe until the poller confirms it, so a stale probe can never
    // leave the line suppressed forever.
    let mut fullscreen_recheck_at: Option<u64> = None;

    loop {
        // Publish the explicit-pin intent for the full-screen poller before blocking again.
        PINNED.store(state.pinned, Ordering::Relaxed);
        let Some(settings_state) = snapshot(app) else {
            log::info!("reveal: application state is gone; stopping the controller");
            break;
        };
        let visible = panel::is_visible();
        let needs_hidden_poll = settings_state.settings.top_edge_reveal
            || state.dismissed_until_leave;
        let active = visible || state.wanted || hide_deadline.is_some();
        let timeout = if capture.active() || active || needs_hidden_poll {
            POLL_INTERVAL
        } else {
            HIDDEN_INTERVAL
        };

        match rx.recv_timeout(timeout) {
            Ok(Command::SuspendForCapture { reply }) => {
                let now = millis(origin);
                let was_visible = panel::is_visible();
                let pending_deadline = hide_deadline.take();
                capture.suspend(was_visible, pending_deadline);
                // Hide through the controller's single window-ownership path (no animation: the
                // overlay covers the screen immediately and the freeze waits for IsWindowVisible).
                panel::hide(app);
                let _ = reply.send(was_visible);
                log::info!(
                    "reveal: capture suspend at +{now} ms (was_visible={was_visible}, pending hide \
                     deadline {pending_deadline:?}); line hidden, other reveal paths suppressed"
                );
            }
            Ok(Command::ResumeAfterCapture { request_new_item }) => {
                let now = millis(origin);
                apply_capture_resume(
                    app,
                    &mut state,
                    &mut hide_deadline,
                    &mut capture,
                    request_new_item,
                    now,
                    &mut left_down,
                    &mut fullscreen_recheck_at,
                );
            }
            Ok(Command::FullscreenSuppressed { suppressed }) => {
                let now = millis(origin);
                apply_fullscreen_state(
                    app,
                    &mut state,
                    &mut hide_deadline,
                    capture.active(),
                    suppressed,
                    now,
                    &mut left_down,
                );
                if !suppressed {
                    // The poller drove the exit (or confirmed normal): its view is authoritative
                    // again, so the controller-side recheck is no longer needed.
                    fullscreen_recheck_at = None;
                }
            }
            Ok(Command::NewItem) => {
                let now = millis(origin);
                if capture.defer_new_item() {
                    log::info!(
                        "reveal: new-item request deferred while the capture overlay is open (+{now} ms)"
                    );
                } else if ready.on_new_item() {
                    pending.push(PendingAction::NewItem);
                    log::info!(
                        "reveal: queued a new-item reveal until the panel reports ready (+{now} ms)"
                    );
                } else if let Some(snapshot) = snapshot(app) {
                    handle_new_item(app, &mut state, &mut hide_deadline, &snapshot, now);
                    left_down = left_button_down();
                }
            }
            Ok(Command::Toggle {
                reason,
                from_pointer,
            }) => {
                let now = millis(origin);
                if capture.active() {
                    log::info!(
                        "reveal: {:?} toggle suppressed while the capture overlay is open (+{now} ms)",
                        reason
                    );
                } else if ready.on_toggle() {
                    pending.push(PendingAction::Toggle {
                        reason,
                        from_pointer,
                    });
                    log::info!(
                        "reveal: queued a {:?} toggle until the panel reports ready (+{now} ms; \
                         {} queued so far)",
                        reason,
                        pending.len()
                    );
                } else {
                    apply_toggle(
                        app,
                        &mut state,
                        &mut hide_deadline,
                        reason,
                        from_pointer,
                        now,
                        last_dismiss,
                        &mut left_down,
                    );
                }
            }
            Ok(Command::DropReveal) => {
                let now = millis(origin);
                if capture.active() {
                    log::info!(
                        "reveal: drop reveal ignored while the capture overlay is open (+{now} ms)"
                    );
                } else if crate::fullscreen::is_suppressed() {
                    log::debug!(
                        "reveal: drop reveal ignored while a full-screen app is active (+{now} ms)"
                    );
                } else if !panel::is_visible() {
                    // The hidden line can sit on another monitor: place it on the pointer's
                    // monitor before showing it, like the top-edge reveal does.
                    let _ = panel::sync_panel_geometry(app, panel::GeometryTarget::Cursor);
                    bounds = None;
                    let item_count = snapshot(app)
                        .map(|snapshot| snapshot.items.len())
                        .unwrap_or(0);
                    state.wanted = true;
                    state.revealed = true;
                    state.pinned = false;
                    state.peek_until = None;
                    state.outside_since = None;
                    state.revealed_at = now;
                    show_and_emit(app, RevealReason::Drop, false, now, item_count);
                    log::info!(
                        "reveal: external drag reached the screen edge; showing the line (+{now} ms)"
                    );
                }
            }
            Ok(Command::DropEnd) => {
                let now = millis(origin);
                log::debug!("reveal: external drag ended (+{now} ms)");
            }
            Ok(Command::Ready) => {
                let now = millis(origin);
                let flush = ready.on_ready();
                if capture.active() {
                    // A capture in the first moments of the app is rare; consume the handshake so the
                    // queue is not stuck, but keep the line hidden under the overlay.
                    log::warn!(
                        "reveal: ready handshake arrived while a capture is suspended; skipping its \
                         startup reveal (+{now} ms)"
                    );
                } else {
                    flush_ready(
                        app,
                        &mut state,
                        &mut hide_deadline,
                        &mut left_down,
                        last_dismiss,
                        &mut pending,
                        &flush,
                        now,
                    );
                }
            }
            Ok(Command::ReadyTimeout) => {
                let now = millis(origin);
                let flush = ready.on_timeout();
                if capture.active() {
                    log::warn!(
                        "reveal: READY timeout while a capture is suspended; keeping the line \
                         hidden (+{now} ms)"
                    );
                } else if flush.already_ready {
                    log::debug!("reveal: READY timeout after the handshake; ignored (+{now} ms)");
                } else {
                    log::warn!(
                        "reveal: panel frontend did not report ready within {} ms; showing the \
                         click-through panel as the fallback (+{now} ms)",
                        READY_TIMEOUT.as_millis()
                    );
                    // The fallback must leave a visible panel even without a frontend animation;
                    // queued actions below may tuck it again (they win, they are the user's). A
                    // full-screen app keeps it hidden (the startup reveal is skipped downstream).
                    if flush.fallback && !crate::fullscreen::is_suppressed() {
                        panel::show(app);
                    }
                    flush_ready(
                        app,
                        &mut state,
                        &mut hide_deadline,
                        &mut left_down,
                        last_dismiss,
                        &mut pending,
                        &flush,
                        now,
                    );
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }

        if capture.active() {
            // The overlay owns the screen: no tuck/hide deadline processing, pointer follow, top-edge
            // dwell or click-outside sampling. The loop keeps polling for the resume command.
            was_visible = false;
            continue;
        }

        let now = millis(origin);
        // A gate entered by the controller's own capture-resume probe (rather than by the poller)
        // is confirmed by the poller within one debounce window. Until it is, re-probe: a stale
        // probe must never leave the line suppressed forever.
        if state.fullscreen_suppressed {
            if let Some(at) = fullscreen_recheck_at {
                if now >= at {
                    match fullscreen_recheck(
                        crate::fullscreen::probe_state(app).is_suppressed(),
                        crate::fullscreen::is_suppressed(),
                    ) {
                        FullscreenRecheck::Confirmed => {
                            // The poller re-confirmed the gate: it will drive the eventual exit.
                            fullscreen_recheck_at = None;
                        }
                        FullscreenRecheck::Retry => {
                            fullscreen_recheck_at =
                                Some(now.saturating_add(FULLSCREEN_RECHECK_MS));
                        }
                        FullscreenRecheck::Restore => {
                            fullscreen_recheck_at = None;
                            log::info!(
                                "reveal: full-screen recheck at +{now} ms found no full-screen app; \
                                 restoring the wish now"
                            );
                            apply_fullscreen_state(
                                app,
                                &mut state,
                                &mut hide_deadline,
                                false,
                                false,
                                now,
                                &mut left_down,
                            );
                        }
                    }
                }
            }
        } else {
            fullscreen_recheck_at = None;
        }

        if let Some(deadline) = hide_deadline {
            if state.wanted {
                hide_deadline = None;
            } else if now >= deadline {
                panel::hide(app);
                hide_deadline = None;
                log::info!("reveal: hidden at +{now} ms (tuck animation finished)");
            }
        }

        let Some(snapshot) = snapshot(app) else {
            break;
        };
        let item_count = snapshot.items.len();
        let now = millis(origin);

        let visible_now = panel::is_visible();
        if visible_now && !was_visible {
            // The panel just became visible: resync on the first visible tick instead of waiting
            // for the next follow poll, so the line opens on the pointer's monitor after a reveal
            // that followed a pointer move or a display change while it was hidden.
            last_monitor_poll = None;
            follow_paused = false;
        }
        was_visible = visible_now;
        if visible_now {
            // Task 14: while visible, follow the pointer across monitors (500 ms debounce) and
            // re-apply the window geometry after a display/DPI change. A hidden panel is never
            // moved; the poll is 200 ms, not per frame.
            //
            // Busy gestures (press/menu, native drag-out) are sampled *before* the follow block:
            // moving or resizing the window mid-OLE-drag (a drag-out that crosses monitors) would
            // yank the source panel under the shell drag. While busy the poll cadence is kept but
            // no monitor is enumerated; a pending candidate is dropped so a full dwell window is
            // required after the gesture, and the next poll re-evaluates.
            let busy = is_busy() || drop_is_held() || dragout::is_dragging();
            let first_poll = last_monitor_poll.is_none();
            let poll_due = last_monitor_poll
                .is_none_or(|last| now.saturating_sub(last) >= panel::FOLLOW_POLL_MS);
            if poll_due {
                last_monitor_poll = Some(now);
                if busy {
                    if !follow_paused {
                        follow_paused = true;
                        log::info!(
                            "panel: monitor follow paused while a press/menu/drag gesture is in \
                             progress"
                        );
                    }
                    // Busy branch of the pure decision helper: drops any pending candidate.
                    let _ = panel::follow_target(&mut follow, None, now, true);
                } else {
                    follow_paused = false;
                    if first_poll {
                        // First visible tick: resync once to the pointer's monitor, so a reveal
                        // after a display change (or a pointer move while the panel was hidden)
                        // opens the line under the user. The debounce below covers changes
                        // observed while the panel is visible.
                        let applied =
                            panel::sync_panel_geometry(app, panel::GeometryTarget::Cursor);
                        if let Some(target) = applied.target.as_ref() {
                            follow.set_applied(&target.id);
                        }
                        follow.display_changed(&panel::monitor_signature(&applied.monitors));
                        if applied.moved {
                            bounds = None;
                            continue;
                        }
                    } else {
                        let monitors = geometry::monitors();
                        // Derive the follow anchor from the window's *actual* monitor on every
                        // poll, not only from the loop's last sync. An external move (manual window
                        // drag, another tool) would otherwise leave the old monitor in `applied`,
                        // making a pointer return to it a no-op until hide/show.
                        if let Some(window) = app
                            .get_webview_window(panel::LABEL)
                            .as_ref()
                            .and_then(panel::window_rect)
                        {
                            if let Some(monitor) = panel::monitor_for_window(&monitors, window) {
                                follow.anchor_to_window(Some(&monitor.id));
                            }
                        }
                        let display_changed =
                            follow.display_changed(&panel::monitor_signature(&monitors));
                        let observed = geometry::cursor_position()
                            .and_then(|point| geometry::monitor_at_point(&monitors, point))
                            .map(|monitor| monitor.id.clone());
                        let followed =
                            panel::follow_target(&mut follow, observed.as_deref(), now, busy);
                        if display_changed || followed.is_some() {
                            if display_changed {
                                log::info!(
                                    "panel: display layout changed ({} monitor(s)); re-syncing \
                                     geometry",
                                    monitors.len()
                                );
                            } else if let Some(id) = followed.as_deref() {
                                log::info!(
                                    "panel: pointer dwelt on {id} for {} ms; following it",
                                    panel::FOLLOW_DEBOUNCE_MS
                                );
                            }
                            // The follow always targets the pointer's monitor; the window's own
                            // monitor is never passed here (that mode is for setting changes).
                            let applied =
                                panel::sync_panel_geometry(app, panel::GeometryTarget::Cursor);
                            if let Some(target) = applied.target.as_ref() {
                                follow.set_applied(&target.id);
                            }
                            // The cached bounds describe the window's old monitor; re-read next
                            // tick.
                            bounds = None;
                            continue;
                        }
                    }
                }
            }

            if let Some(current) = read_bounds(app) {
                bounds = Some(current);
            }
            let Some(panel_bounds) = bounds else {
                continue;
            };
            let cursor = geometry::cursor_position();
            let inside_panel = cursor.is_some_and(|point| panel_bounds.panel.contains(point));
            let inside_band = cursor.is_some_and(|point| panel_bounds.band.contains(point));
            let down = left_button_down();

            if state.revealed && down && !left_down && !inside_panel && !busy {
                if on_click_outside(&mut state, now) {
                    last_dismiss = Some(now);
                    tuck_and_emit(app, TuckReason::Dismiss, now, busy, None);
                    hide_deadline = Some(now.saturating_add(TUCK_ANIMATION_MS));
                    log::info!(
                        "reveal: click outside the panel -> dismissed until the pointer leaves \
                         the band (at +{now} ms)"
                    );
                }
            }
            left_down = down;

            if state.revealed {
                let outside_since = state.outside_since;
                if let Some(reason) =
                    tick(&mut state, now, item_count, inside_band, busy)
                {
                    let outside_ms = outside_since
                        .map(|since| now.saturating_sub(since));
                    tuck_and_emit(app, reason, now, busy, outside_ms);
                    hide_deadline = Some(now.saturating_add(TUCK_ANIMATION_MS));
                }
            }
        } else {
            if was_visible {
                // The line just tucked: drop the hidden-poll monitor cache so the next hidden
                // tick re-enumerates immediately (hotplug while tucked is picked up then).
                edge_monitors = None;
            }
            let top_edge_monitors = if snapshot.settings.top_edge_reveal
                && !snapshot.items.is_empty()
            {
                let due = edge_monitors.as_ref().is_none_or(|(_, at)| {
                    now.saturating_sub(*at) >= HIDDEN_EDGE_MONITOR_REFRESH_MS
                });
                if due {
                    edge_monitors = Some((geometry::monitors(), now));
                }
                edge_monitors.as_ref().map(|(monitors, _)| monitors.as_slice())
            } else {
                edge_monitors = None;
                None
            };
            hidden_tick(
                app,
                &mut state,
                &snapshot,
                bounds,
                top_edge_monitors,
                now,
                &mut left_down,
            );
        }
    }
}

/// Handles a watcher's new-item request (re-checks the setting and the empty-line guard).
fn handle_new_item(
    app: &AppHandle,
    state: &mut State,
    hide_deadline: &mut Option<u64>,
    snapshot: &PublicState,
    now: u64,
) {
    if state.fullscreen_suppressed {
        // The line must not flash over the full-screen app. Remember that an item arrived; the
        // exit path shows one deferred peek when `revealOnNew` is on (the newest cards are all
        // visible then, so counting is enough).
        state.fullscreen_deferred_new_items =
            state.fullscreen_deferred_new_items.saturating_add(1);
        log::info!(
            "reveal: new item deferred while full-screen suppression is active ({} deferred, \
             +{now} ms)",
            state.fullscreen_deferred_new_items
        );
        return;
    }
    let item_count = snapshot.items.len();
    if item_count == 0 {
        log::info!("reveal: ignored a newItem request on an empty line");
        return;
    }
    if !snapshot.settings.reveal_on_new {
        log::debug!("reveal: ignored a newItem request because revealOnNew is off");
        return;
    }
    let peek_ms = u64::from(snapshot.settings.reveal_peek_ms);
    // The default path goes through the documented two-argument interface; only a customized
    // `revealPeekMs` needs the explicit duration.
    let should_show = if peek_ms == PEEK_MS {
        on_new_item(state, now)
    } else {
        on_new_item_for(state, now, peek_ms)
    };
    if should_show {
        *hide_deadline = None;
        show_and_emit(app, RevealReason::NewItem, true, now, item_count);
    } else if state.dismissed_until_leave {
        log::info!("reveal: new-item peek suppressed until the pointer leaves the band");
    } else {
        log::debug!("reveal: line already revealed; peek extended to +{} ms", state.peek_until.unwrap_or(now));
    }
}

/// Hidden-panel work: clear a stale dismiss suppression and (optionally) watch the top edge.
///
/// The top edge is checked against the monitor under the pointer (`monitors`), not the panel's
/// current monitor: hiding the line must not pin the hover trigger to the display it was last
/// shown on. `monitors` is a cached enumeration owned by the loop (refreshed once per second
/// while the line is hidden), because the hidden poll runs at 30 Hz.
fn hidden_tick(
    app: &AppHandle,
    state: &mut State,
    snapshot: &PublicState,
    bounds: Option<PanelBounds>,
    monitors: Option<&[geometry::MonitorInfo]>,
    now: u64,
    left_down: &mut bool,
) {
    if state.fullscreen_suppressed {
        // A full-screen app owns the screen: no top-edge dwell, no pointer bookkeeping that could
        // re-reveal the line. The dismiss flag was already cleared by `on_fullscreen_enter`.
        return;
    }
    let cursor = geometry::cursor_position();

    if state.dismissed_until_leave {
        match (bounds, cursor) {
            (Some(panel_bounds), Some(point)) => {
                if !panel_bounds.band.contains(point) {
                    state.dismissed_until_leave = false;
                    log::debug!("reveal: pointer left the band; dismiss suppression cleared");
                }
            }
            // Without a known band the suppression can never be resolved; do not block the line
            // forever (should only happen if the window was never shown).
            _ => state.dismissed_until_leave = false,
        }
    }

    let at_edge = if snapshot.settings.top_edge_reveal
        && !snapshot.items.is_empty()
        && bounds.is_some()
    {
        // `bounds` is the "the line has been shown at least once" signal: the window is created
        // hidden to avoid a first-paint flash, so an early top-edge reveal before the READY
        // handshake would show an unpainted window. The first successful show fills the bounds.
        match (monitors, cursor) {
            (Some(monitors), Some(point)) => {
                panel::top_edge_target(monitors, point, TOP_EDGE_BAND_DP).is_some()
            }
            _ => false,
        }
    } else {
        state.top_edge_since = None;
        false
    };

    if on_top_edge(state, now, at_edge) {
        // Place the line on the monitor whose top edge was hovered *before* showing it, so the
        // reveal does not flash on the panel's previous display and then jump.
        panel::sync_panel_geometry(app, panel::GeometryTarget::Cursor);
        show_and_emit(app, RevealReason::TopEdge, false, now, snapshot.items.len());
        *left_down = left_button_down();
    }
}

/// Takes a full snapshot, tolerating shutdown (the managed state may already be dropped).
fn snapshot(app: &AppHandle) -> Option<PublicState> {
    app.try_state::<AppState>().map(|state| state.snapshot())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A state right after a new item peek started at `t = 0`.
    fn peeked() -> State {
        let mut state = State::default();
        assert!(on_new_item(&mut state, 0));
        state
    }

    #[test]
    fn new_item_peeks_then_tucks_after_delay() {
        let mut state = State::default();
        assert!(on_new_item(&mut state, 0));
        assert!(state.wanted && state.revealed);
        assert_eq!(state.peek_until, Some(PEEK_MS));

        // The peek protects the line until it expires, even with the pointer already away.
        assert!(!on_pointer_leave(&mut state, 1000));
        assert!(state.revealed);

        // Peek over at 2500 ms and the pointer has been away long enough -> tuck.
        assert!(on_pointer_leave(&mut state, 2600));
        assert!(!state.wanted && !state.revealed);
    }

    #[test]
    fn toggle_pins_the_line_and_ignores_pointer_leave_and_empty_timeout() {
        let mut state = State::default();
        assert!(on_toggle(&mut state, 100));
        assert!(state.wanted && state.revealed && state.pinned);

        assert!(!on_pointer_leave(&mut state, 1000));
        assert_eq!(tick(&mut state, 7000, 0, false, false), None);
        assert!(state.revealed, "a pinned line never auto-hides");

        assert!(!on_toggle(&mut state, 8000));
        assert!(!state.wanted && !state.revealed);
    }

    #[test]
    fn explicit_toggle_reveals_an_empty_line() {
        let mut state = State::default();
        assert!(on_toggle(&mut state, 10));
        assert!(state.wanted && state.revealed && state.pinned);
    }

    #[test]
    fn empty_line_auto_hides_after_700ms() {
        let mut state = State::default();
        on_ready(&mut state, 1000);
        assert_eq!(tick(&mut state, 1699, 0, true, false), None);
        assert_eq!(
            tick(&mut state, 1700, 0, true, false),
            Some(TuckReason::Empty)
        );
        assert!(!state.wanted);
    }

    #[test]
    fn click_outside_dismisses_until_the_pointer_leaves_the_band() {
        let mut state = peeked();

        assert!(on_click_outside(&mut state, 100));
        assert!(!state.revealed && state.dismissed_until_leave);

        // A peek is suppressed while the pointer is still in the trigger band.
        assert!(!on_new_item(&mut state, 200));
        assert_eq!(tick(&mut state, 300, 1, true, false), None);
        assert!(state.dismissed_until_leave);

        // Leaving the band clears the suppression; the next peek reveals again.
        assert_eq!(tick(&mut state, 400, 1, false, false), None);
        assert!(!state.dismissed_until_leave);
        assert!(on_new_item(&mut state, 500));
        assert!(state.revealed);
    }

    #[test]
    fn busy_gestures_prevent_the_pointer_leave_tuck() {
        let mut state = peeked(); // peek ends at 2500 ms

        assert_eq!(tick(&mut state, 2600, 1, false, true), None);
        assert_eq!(tick(&mut state, 3200, 1, false, true), None);
        assert!(state.revealed, "menu/drag/pressed must not tuck");

        // The 500 ms outside timer starts after the busy gesture ends.
        assert_eq!(tick(&mut state, 3200, 1, false, false), None);
        assert_eq!(tick(&mut state, 3699, 1, false, false), None);
        assert_eq!(
            tick(&mut state, 3700, 1, false, false),
            Some(TuckReason::PointerLeave)
        );
    }

    #[test]
    fn pointer_leave_is_debounced_by_500ms() {
        let mut state = peeked();
        assert!(!on_pointer_leave(&mut state, 2500));
        assert!(!on_pointer_leave(&mut state, 2999));
        assert!(on_pointer_leave(&mut state, 3000));
    }

    #[test]
    fn top_edge_reveal_needs_a_500ms_dwell() {
        let mut state = State::default();
        assert!(!on_top_edge(&mut state, 100, true));
        assert!(!on_top_edge(&mut state, 599, true));
        assert!(on_top_edge(&mut state, 600, true));
        assert!(state.revealed && !state.pinned);

        // Leaving the edge resets the dwell window.
        let mut state = State::default();
        assert!(!on_top_edge(&mut state, 0, true));
        assert!(!on_top_edge(&mut state, 100, false));
        assert!(!on_top_edge(&mut state, 700, true));
        assert!(on_top_edge(&mut state, 1200, true));
    }

    #[test]
    fn pointer_toggle_right_after_a_click_dismiss_stays_hidden() {
        // Clicking the tray icon/menu while the line is visible first dismisses the line; the
        // toggle that the same click produces must not immediately reveal it again.
        let mut dismissed = peeked();
        assert!(on_click_outside(&mut dismissed, 1000));
        assert!(pointer_toggle_was_the_dismiss(
            &dismissed,
            Some(1000),
            1100
        ));
        assert!(!pointer_toggle_was_the_dismiss(
            &dismissed,
            Some(1000),
            2000
        ));

        // A visible line has not been dismissed, so a pointer toggle is a normal toggle.
        let mut visible = State::default();
        on_ready(&mut visible, 500);
        assert!(!pointer_toggle_was_the_dismiss(&visible, Some(1000), 1100));
    }

    /// Before the handshake every toggle is queued; after it, toggles apply immediately.
    #[test]
    fn ready_queue_holds_toggles_until_the_handshake() {
        let mut queue = ReadyQueue::default();
        assert!(!queue.is_ready());
        assert!(queue.on_toggle(), "a pre-ready toggle is queued");
        assert!(queue.on_toggle());
        assert_eq!(queue.queued_toggles(), 2);

        let flush = queue.on_ready();
        assert!(queue.is_ready());
        assert_eq!(
            flush,
            ReadyFlush {
                baseline: false,
                toggles: 2,
                new_items: 0,
                fallback: false,
                already_ready: false,
            },
            "on_ready must not clobber the queued toggles (no baseline reveal)"
        );
        assert_eq!(queue.queued_toggles(), 0, "the queue is drained");
        assert!(!queue.on_toggle(), "after the handshake a toggle applies live");
    }

    /// An empty queue on the handshake runs the normal startup reveal; new-item reveals are queued
    /// and counted separately from toggles.
    #[test]
    fn ready_queue_baseline_and_new_items() {
        let mut queue = ReadyQueue::default();
        let baseline = queue.on_ready();
        assert_eq!(
            baseline,
            ReadyFlush {
                baseline: true,
                ..ReadyFlush::default()
            }
        );

        let mut queue = ReadyQueue::default();
        assert!(queue.on_new_item());
        assert!(queue.on_new_item());
        assert_eq!(queue.queued_new_items(), 2);
        let flush = queue.on_ready();
        assert!(!flush.baseline);
        assert_eq!(flush.toggles, 0);
        assert_eq!(flush.new_items, 2);
    }

    /// The 3 s fallback unblocks the queue exactly like the handshake but marks the flush so the
    /// controller shows the click-through panel; a late handshake is a logged duplicate.
    #[test]
    fn ready_timeout_is_the_fallback_and_a_late_ready_is_ignored() {
        let mut queue = ReadyQueue::default();
        assert!(queue.on_toggle());
        let timeout = queue.on_timeout();
        assert!(timeout.fallback);
        assert!(!timeout.baseline);
        assert_eq!(timeout.toggles, 1);
        assert!(!timeout.already_ready);

        assert_eq!(
            queue.on_ready(),
            ReadyFlush {
                already_ready: true,
                ..ReadyFlush::default()
            },
            "the real handshake after the fallback must be ignored"
        );
        assert_eq!(
            queue.on_timeout(),
            ReadyFlush {
                already_ready: true,
                fallback: true,
                ..ReadyFlush::default()
            },
            "a second timeout is idempotent"
        );
    }

    /// A toggle queued before the handshake is replayed with the same state-machine result: one
    /// toggle on a hidden line reveals it pinned; a second toggle tucks it again.
    #[test]
    fn queued_toggle_parity_replay_matches_live_toggle_semantics() {
        let mut queue = ReadyQueue::default();
        assert!(queue.on_toggle());
        let flush = queue.on_ready();
        assert_eq!(flush.toggles, 1);
        // The controller replays each queued toggle with `on_toggle`; verify the pure transitions.
        let mut state = State::default();
        assert!(on_toggle(&mut state, 100));
        assert!(state.revealed && state.wanted && state.pinned);
        assert!(!on_toggle(&mut state, 200));
        assert!(!state.revealed && !state.wanted && !state.pinned);
    }

    // ---------------------------------------------------------------------------------------
    // Task 16 review: capture-overlay suspension (pure gate + resume deadline decisions)
    // ---------------------------------------------------------------------------------------

    #[test]
    fn capture_gate_records_and_resumes_the_pre_capture_visibility_once() {
        let mut gate = CaptureGate::default();
        assert!(!gate.active());

        let suspension = gate.suspend(true, Some(1234));
        assert_eq!(
            suspension,
            CaptureSuspension {
                was_visible: true,
                hide_deadline: Some(1234),
            }
        );
        assert!(gate.active(), "the overlay owns the line while suspended");
        assert!(
            gate.defer_new_item(),
            "a new item must be deferred while suspended"
        );

        let resume = gate.resume().expect("the suspension is resumed");
        assert_eq!(resume.suspension, suspension);
        assert!(resume.queued_new_item);
        assert!(!gate.active(), "the gate is released");
        assert!(gate.resume().is_none(), "resume happens exactly once");
        assert!(
            !gate.defer_new_item(),
            "no new item is deferred after the resume"
        );
    }

    #[test]
    fn capture_gate_only_defers_new_items_while_active() {
        let mut gate = CaptureGate::default();
        assert!(!gate.defer_new_item(), "no capture: the watcher path stays live");
        gate.suspend(false, None);
        assert!(gate.defer_new_item());
        assert!(gate.defer_new_item(), "repeats are idempotent (one deferred reveal)");
        let resume = gate.resume().expect("resume");
        assert!(!resume.suspension.was_visible);
        assert!(resume.queued_new_item);
    }

    #[test]
    fn resume_restores_the_pending_tuck_deadline_only_when_the_line_was_visible() {
        assert_eq!(resume_hide_plan(false, None, 5000), ResumeHide::None);
        assert_eq!(
            resume_hide_plan(false, Some(9000), 5000),
            ResumeHide::None,
            "a hidden line has nothing to restore (the deadline is not applied)"
        );
        assert_eq!(
            resume_hide_plan(true, None, 5000),
            ResumeHide::None,
            "a visible line with no pending tuck is simply shown again"
        );
        assert_eq!(
            resume_hide_plan(true, Some(9000), 5000),
            ResumeHide::Pending(9000),
            "a future tuck deadline is restored so the line cannot stay up forever"
        );
        assert_eq!(
            resume_hide_plan(true, Some(5000), 6001),
            ResumeHide::HideNow,
            "a deadline that elapsed during the overlay keeps the line hidden"
        );
    }

    // ---------------------------------------------------------------------------------------
    // P4-2/M4: full-screen suppression (pure state transitions)
    // ---------------------------------------------------------------------------------------

    /// Entering while visible records the wish, hides the state and keeps the pin for the restore.
    #[test]
    fn fullscreen_suppression_records_and_restores_a_visible_peek() {
        let mut state = peeked(); // revealed, not pinned, peek at 2500 ms
        assert!(on_fullscreen_enter(&mut state));
        assert!(!state.wanted && !state.revealed, "the line is hidden");
        assert_eq!(state.wish_before_suppression, Some(true));
        assert!(!state.pinned, "a peek stays a peek, not a pin");
        assert_eq!(state.peek_until, None, "the old peek deadline is gone");

        let restore = on_fullscreen_exit(&mut state);
        assert_eq!(
            restore,
            FullscreenRestore {
                was_wanted: true,
                pinned: false,
                deferred_new_items: 0,
            }
        );
        assert!(!state.fullscreen_suppressed);
        assert_eq!(state.wish_before_suppression, None, "the wish is consumed once");
        assert_eq!(
            on_fullscreen_exit(&mut state),
            FullscreenRestore::default(),
            "exit is idempotent"
        );
    }

    /// A pinned line keeps its pin across the suppression and restores as pinned.
    #[test]
    fn fullscreen_suppression_preserves_a_pinned_line() {
        let mut state = State::default();
        assert!(on_toggle(&mut state, 100));
        assert!(on_fullscreen_enter(&mut state));
        assert!(!state.revealed);
        assert!(state.pinned, "the pinned intent survives the suppression");

        let restore = on_fullscreen_exit(&mut state);
        assert!(restore.was_wanted && restore.pinned);
        // The controller restores a pinned line through the normal toggle transition.
        assert!(on_toggle(&mut state, 5000));
        assert!(state.revealed && state.pinned);
    }

    /// A hidden line (the normal case when a game starts) has no restore wish.
    #[test]
    fn fullscreen_suppression_while_hidden_has_no_restore_wish() {
        let mut state = State::default();
        assert!(!on_fullscreen_enter(&mut state), "nothing to tuck");
        assert_eq!(state.wish_before_suppression, Some(false));
        let restore = on_fullscreen_exit(&mut state);
        assert!(!restore.was_wanted && !restore.pinned);
    }

    /// Entering twice is a no-op (a duplicated poll command must not re-record the wish).
    #[test]
    fn fullscreen_suppression_enter_is_idempotent_and_never_sticks() {
        let mut state = peeked();
        assert!(on_fullscreen_enter(&mut state));
        assert!(!on_fullscreen_enter(&mut state), "the second enter is a no-op");
        assert_eq!(state.wish_before_suppression, Some(true));
        let restore = on_fullscreen_exit(&mut state);
        assert!(restore.was_wanted);

        // A second cycle starts from the restored state and ends normally.
        assert!(on_new_item(&mut state, 10_000));
        assert!(on_fullscreen_enter(&mut state));
        assert!(on_fullscreen_exit(&mut state).was_wanted);
        assert!(!state.fullscreen_suppressed);
    }

    /// A suppressed toggle always pins the line; the override only opens the gate with both the pin
    /// and the setting.
    #[test]
    fn fullscreen_suppressed_toggle_pins_and_requires_the_setting_for_the_override() {
        let mut state = State::default();
        on_fullscreen_enter(&mut state); // starts hidden: wish Some(false)

        // Setting off: the toggle pins the line, stays hidden and records the restore wish.
        assert!(!on_suppressed_toggle(&mut state, false));
        assert!(state.pinned && state.fullscreen_suppressed);
        assert_eq!(state.wish_before_suppression, Some(true));

        // A second toggle while suppressed is idempotent: the line is not visible, so there is
        // nothing to un-pin; the restore wish must not be lost.
        assert!(!on_suppressed_toggle(&mut state, false));
        assert!(state.pinned);
        assert_eq!(state.wish_before_suppression, Some(true));

        // With the setting on the same toggle is the spec's explicit override: the gate opens.
        assert!(on_suppressed_toggle(&mut state, true));
        assert!(state.pinned);
        assert!(!state.fullscreen_suppressed, "the controller may show the line now");
        assert_eq!(state.wish_before_suppression, None);
        assert_eq!(state.fullscreen_deferred_new_items, 0);
    }

    /// Deferred new items are counted while suppressed and taken exactly once on exit.
    #[test]
    fn fullscreen_deferred_new_items_are_taken_once() {
        let mut state = State::default();
        on_fullscreen_enter(&mut state);
        state.fullscreen_deferred_new_items = 2;
        let restore = on_fullscreen_exit(&mut state);
        assert_eq!(restore.deferred_new_items, 2);
        assert_eq!(state.fullscreen_deferred_new_items, 0);
        assert!(!on_fullscreen_exit(&mut state).was_wanted);
    }

    /// The visible-reveal timers are cleared even when the line was already hidden, so a stale peek
    /// or dwell timestamp cannot fire right after the restore.
    #[test]
    fn fullscreen_enter_clears_the_visible_reveal_timers_unconditionally() {
        let mut state = State::default();
        state.peek_until = Some(9_999);
        state.outside_since = Some(100);
        state.top_edge_since = Some(200);
        assert!(!on_fullscreen_enter(&mut state), "a hidden line has nothing to tuck");
        assert_eq!(state.peek_until, None);
        assert_eq!(state.outside_since, None);
        assert_eq!(state.top_edge_since, None);
        assert_eq!(state.wish_before_suppression, Some(false));
    }

    /// A startup reveal skipped because a game was already running is replayed after the game.
    #[test]
    fn fullscreen_startup_wish_replays_the_skipped_startup_reveal() {
        let mut state = State::default();
        on_fullscreen_startup(&mut state);
        assert!(state.fullscreen_suppressed);
        assert_eq!(state.wish_before_suppression, Some(true));
        let restore = on_fullscreen_exit(&mut state);
        assert!(restore.was_wanted, "the skipped startup reveal is not lost");
        assert!(!restore.pinned);
        assert!(!state.fullscreen_suppressed);
    }

    // ---------------------------------------------------------------------------------------
    // P4-2 review fix: capture resume re-probes the full-screen gate
    // ---------------------------------------------------------------------------------------

    /// Every recorded/probed gate combination maps to exactly one capture-resume decision.
    #[test]
    fn capture_resume_fullscreen_plan_matrix() {
        // No full-screen app before or after the overlay: the normal capture resume runs.
        assert_eq!(
            capture_fullscreen_plan(false, false),
            CaptureFullscreenPlan::Normal
        );
        // The overlay hid a still-full-screen app and the poller had applied "normal": enter the
        // gate again instead of re-showing the line over the game.
        assert_eq!(
            capture_fullscreen_plan(false, true),
            CaptureFullscreenPlan::StaySuppressed { enter: true }
        );
        // The gate is still recorded and the app is still there: stay hidden.
        assert_eq!(
            capture_fullscreen_plan(true, true),
            CaptureFullscreenPlan::StaySuppressed { enter: false }
        );
        // The app went away during the overlay: exit the recorded gate and restore the wish (the
        // poller already applied "normal", so no poller event would ever do it).
        assert_eq!(
            capture_fullscreen_plan(true, false),
            CaptureFullscreenPlan::Restore
        );
    }

    /// The controller-side recheck never stays suppressed when the app is gone, even if the
    /// poller's applied state still says "suppressed" (no poller event may ever arrive).
    #[test]
    fn fullscreen_recheck_restores_without_waiting_for_a_poller_event() {
        assert_eq!(
            fullscreen_recheck(false, false),
            FullscreenRecheck::Restore,
            "the probe alone can end a gate the controller entered"
        );
        assert_eq!(
            fullscreen_recheck(false, true),
            FullscreenRecheck::Restore,
            "the app is gone even though the poller still believes it is suppressed"
        );
        assert_eq!(
            fullscreen_recheck(true, true),
            FullscreenRecheck::Confirmed,
            "once the poller confirms, it drives the eventual exit"
        );
        assert_eq!(
            fullscreen_recheck(true, false),
            FullscreenRecheck::Retry,
            "a probe-only gate is retried until the poller catches up"
        );
    }
}
