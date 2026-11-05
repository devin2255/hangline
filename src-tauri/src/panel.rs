//! The Hangline top panel: a borderless, transparent, always-on-top "clothesline" strip.
//!
//! The panel geometry comes from the target monitor's work area (design spec §3.3): the strip is
//! exactly as wide as the work area and `photoMaxHeight(size) + 106` dp tall, placed at the work
//! area's top-left. Width and height are applied in physical pixels, so a 150% monitor gets the
//! same physical coverage as a 100% one and the webview reports the matching logical width.

use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;
use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder,
};

use crate::app_state::AppState;
use crate::geometry::{self, MonitorInfo};
use crate::settings::CardSize;
use crate::tray;

/// Window label used by the frontend (`?window=panel`) and by the hit-test loop.
pub const LABEL: &str = "panel";

/// Everything around the photo: rope/pin gap, card chrome and the transparent margin below the
/// line (design spec §3.3: `panelHeight = photoMaxHeight + 106`).
const PANEL_CHROME_DP: u32 = 106;

/// Fallback strip when no monitor can be enumerated (no desktop: should not happen in production).
const FALLBACK_WIDTH_DP: f64 = 1440.0;
const FALLBACK_HEIGHT_DP: f64 = 210.0;

/// Whether the panel is meant to be on screen.
///
/// [`show`]/[`hide`] maintain this flag; it is the gate the 30 Hz hit-test loop reads, so a
/// hidden panel costs no window queries at all (design spec 4.6: stop polling while hidden).
static VISIBLE: AtomicBool = AtomicBool::new(false);

/// How long the pointer must stay on another monitor before the panel follows it (Task 14).
///
/// The user may sweep the pointer across a monitor seam on the way to a menu; a 500 ms dwell
/// avoids chasing every crossing, and the follow is polled at [`FOLLOW_POLL_MS`], never per frame.
pub const FOLLOW_DEBOUNCE_MS: u64 = 500;

/// How often the visible reveal controller re-checks the monitor layout (Task 14).
///
/// The dwell in [`FOLLOW_DEBOUNCE_MS`] starts at the first sample that sees the new monitor, so a
/// 200 ms poll keeps the observed latency from a completed 500 ms dwell to at most ~700 ms until
/// the window moves. The window geometry itself is only touched when a poll decides to follow.
pub const FOLLOW_POLL_MS: u64 = 200;

/// Which monitor a [`sync_panel_geometry`] call re-fits (Task 14 review fix).
///
/// The split matters: a setting change (`set_setting`) or the explicit frontend sync must re-fit
/// the monitor the window is already on, otherwise changing `cardSize` from the settings window on
/// another display teleports the line across the desktop. Only the reveal follow loop places the
/// panel on the pointer's monitor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GeometryTarget {
    /// Re-fit the monitor the panel window currently occupies (primary when there is no window).
    CurrentPanel,
    /// Place the panel on the monitor under the pointer: the debounced reveal follow, and the
    /// top-edge reveal's pre-show sync (both run only while the line is hidden/being revealed).
    Cursor,
}

/// Physical window position and inner size: the pair the follow anchor derives from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowRect {
    /// Window origin in physical virtual-desktop pixels.
    pub position: (i32, i32),
    /// Window inner size in physical pixels (the panel is undecorated, so inner == outer).
    pub size: (u32, u32),
}

/// Window size in physical pixels, serialized for the `sync_panel_geometry` reply.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PixelSize {
    pub width: u32,
    pub height: u32,
}

/// Window origin in physical virtual-desktop pixels, serialized for the reply.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct PixelPosition {
    pub x: i32,
    pub y: i32,
}

/// Reply of the `sync_panel_geometry` command (Task 14).
///
/// It carries the full monitor list and the monitor Rust chose, so the frontend can log all
/// monitors plus the actual target instead of guessing from the primary. `size`/`position` are the
/// window geometry that was applied (or that already matched); `moved` is false for a no-op sync.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PanelGeometry {
    /// Every monitor of the virtual desktop, left-to-right.
    pub monitors: Vec<MonitorInfo>,
    /// Monitor the panel was placed on (the window's monitor for a re-fit, the cursor's monitor
    /// for the reveal follow).
    pub target: Option<MonitorInfo>,
    /// Physical window size the panel has on `target`; `None` when there is no window to apply it
    /// to (the reply never reports desired-but-not-applied geometry).
    pub size: Option<PixelSize>,
    /// Physical window origin (`work_area` top-left) on `target`; `None` when there is no window.
    pub position: Option<PixelPosition>,
    /// Whether the window API calls were actually needed (false = geometry already matched).
    pub moved: bool,
}

/// Debounced pointer-follow state for the panel's monitor (Task 14).
///
/// Pure and time-injected: [`observe`](Self::observe) takes `now` in milliseconds and returns the
/// monitor id once the pointer has dwelt on it for the debounce window and it differs from
/// [`applied`](Self::applied). [`display_changed`](Self::display_changed) tracks the monitor layout
/// signature so a hotplug/resolution/DPI change re-syncs the panel even when the pointer is still.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MonitorFollow {
    /// Monitor the panel currently targets (`None` until the first sample adopts it).
    applied: Option<String>,
    /// Candidate monitor and the time it was first seen.
    pending: Option<(String, u64)>,
    /// Last observed display-layout signature.
    display: Option<String>,
}

impl MonitorFollow {
    /// Anchors the state to a known monitor (used after a display change re-synced the window).
    pub fn set_applied(&mut self, id: &str) {
        self.applied = Some(id.to_string());
        self.pending = None;
    }

    /// Adopts `id` when the window has actually moved to it (external move detection).
    ///
    /// The reveal loop calls this every follow poll with the monitor derived from the window's real
    /// position. Without it, `applied` only ever reflected the loop's own last sync, so an
    /// externally moved panel kept the old monitor as the anchor and a pointer return to that
    /// monitor was short-circuited by [`observe`](Self::observe) forever (review finding).
    /// A monitor change also drops a pending candidate: the new anchor is a reset, not a follow.
    pub fn anchor_to_window(&mut self, id: Option<&str>) {
        let Some(id) = id else {
            return;
        };
        if self.applied.as_deref() != Some(id) {
            self.applied = Some(id.to_string());
            self.pending = None;
        }
    }

    /// Drops a pending candidate without moving the window (a busy gesture owns the mouse).
    fn reset_pending(&mut self) {
        self.pending = None;
    }

    /// Records a display-layout signature; returns `true` exactly when it changed.
    ///
    /// The first call only records the baseline (the startup geometry was just applied), so a
    /// fresh controller does not re-sync on its first tick.
    pub fn display_changed(&mut self, signature: &str) -> bool {
        match self.display.as_deref() {
            Some(current) if current == signature => false,
            None => {
                self.display = Some(signature.to_string());
                false
            }
            Some(_) => {
                self.display = Some(signature.to_string());
                self.pending = None;
                true
            }
        }
    }

    /// Observes the monitor under the pointer at `now` (milliseconds).
    ///
    /// Returns `Some(id)` once `id` has been the observed monitor for at least `debounce_ms` and is
    /// not the applied monitor; the call also moves the applied anchor. The first observation
    /// adopts the monitor without returning it (the window was just created there). `None` (a
    /// cursor in a seam, or a failed query) cancels a pending move.
    pub fn observe(&mut self, observed: Option<&str>, now: u64, debounce_ms: u64) -> Option<String> {
        let Some(id) = observed else {
            self.pending = None;
            return None;
        };
        if self.applied.as_deref() == Some(id) {
            self.pending = None;
            return None;
        }
        if self.applied.is_none() {
            // Startup: the panel was created on the cursor's monitor already.
            self.applied = Some(id.to_string());
            self.pending = None;
            return None;
        }
        match &self.pending {
            Some((pending_id, since)) if pending_id == id => {
                if now.saturating_sub(*since) < debounce_ms {
                    return None;
                }
                self.applied = Some(id.to_string());
                self.pending = None;
                Some(id.to_string())
            }
            _ => {
                self.pending = Some((id.to_string(), now));
                None
            }
        }
    }
}

/// Card scale per size (design spec §3.3: 0.8 / 1.0 / 1.35).
fn card_scale(size: CardSize) -> f64 {
    match size {
        CardSize::Small => 0.8,
        CardSize::Medium => 1.0,
        CardSize::Large => 1.35,
    }
}

/// Maximum photo height in dp: 83 / 104 / 140 for small / medium / large (design spec §3.3).
pub fn photo_max_height(size: CardSize) -> u32 {
    (104.0 * card_scale(size)).round() as u32
}

/// Total panel height in dp: 189 / 210 / 246 (design spec §3.3).
pub fn panel_height(size: CardSize) -> u32 {
    photo_max_height(size) + PANEL_CHROME_DP
}

/// Physical window size that covers the monitor work area exactly.
///
/// Width is the work area width unchanged (physical pixels); height is the dp height scaled by
/// the monitor's effective DPI. A degenerate work area still yields a 1 px window, and an
/// invalid scale factor falls back to 100%.
pub fn panel_pixel_size(monitor: &MonitorInfo, size: CardSize) -> PhysicalSize<u32> {
    let width = u32::try_from(monitor.work_area.w.max(1)).unwrap_or(1);
    let height = u32::try_from(
        geometry::dp_to_px(f64::from(panel_height(size)), valid_scale(monitor)).max(1),
    )
    .unwrap_or(1);
    PhysicalSize::new(width, height)
}

/// Monitor scale factor, falling back to 100% for corrupt/zero values.
fn valid_scale(monitor: &MonitorInfo) -> f64 {
    if monitor.scale_factor.is_finite() && monitor.scale_factor > 0.0 {
        monitor.scale_factor
    } else {
        1.0
    }
}

/// Picks the monitor the panel should live on: the one under the cursor, else the primary.
///
/// The cursor lookup is injected so the rule is unit-testable; [`target_monitor`] is the live
/// wrapper (window creation), and [`select_target`] uses the same rule for
/// [`GeometryTarget::Cursor`] (the debounced reveal follow).
pub fn target_monitor_at(
    monitors: &[MonitorInfo],
    cursor: Option<(i32, i32)>,
) -> Option<MonitorInfo> {
    cursor
        .and_then(|point| geometry::monitor_at_point(monitors, point).cloned())
        .or_else(|| monitors.iter().find(|monitor| monitor.is_primary).cloned())
        .or_else(|| monitors.first().cloned())
}

/// Live target: the monitor under the pointer, with primary/first fallbacks.
pub fn target_monitor(monitors: &[MonitorInfo]) -> Option<MonitorInfo> {
    target_monitor_at(monitors, geometry::cursor_position())
}

/// Whether `point` rests on the work-area top edge of `monitor`, within `tolerance_dp`.
///
/// The top-edge reveal is per monitor: the panel may currently live on another display, so the
/// hit test must be expressed against the monitor under the pointer, not the window's monitor.
/// The tolerance is a dp value scaled by the monitor's own DPI (2 dp is 2/3 px at 100/150%), and
/// the work area (taskbar excluded) is the edge the panel actually hangs from.
pub fn top_edge_hit(monitor: &MonitorInfo, point: (i32, i32), tolerance_dp: f64) -> bool {
    let area = monitor.work_area;
    let tolerance = geometry::dp_to_px(tolerance_dp, monitor.scale_factor);
    point.0 >= area.x
        && point.0 < area.x.saturating_add(area.w)
        && point.1 >= area.y
        && point.1 <= area.y.saturating_add(tolerance)
}

/// Monitor whose top edge the pointer is on, if any (pure).
///
/// This is what makes top-edge reveal work across displays: the pointer may be on a monitor the
/// panel is not currently placed on, so the lookup must not use the panel's own bounds.
pub fn top_edge_target<'a>(
    monitors: &'a [MonitorInfo],
    point: (i32, i32),
    tolerance_dp: f64,
) -> Option<&'a MonitorInfo> {
    geometry::monitor_at_point(monitors, point)
        .filter(|monitor| top_edge_hit(monitor, point, tolerance_dp))
}

/// Reads the panel window's physical rectangle for the monitor derivation.
///
/// `None` when the window is gone or a query fails (shutdown, a destroyed webview); callers then
/// fall back instead of guessing a monitor.
pub fn window_rect(win: &WebviewWindow) -> Option<WindowRect> {
    let position = win.outer_position().ok()?;
    let size = win.inner_size().ok()?;
    Some(WindowRect {
        position: (position.x, position.y),
        size: (size.width, size.height),
    })
}

/// Monitor containing the centre of `window`'s rectangle.
///
/// The centre (not the origin) is used so a window that still overhangs a seam right after a move
/// is attributed to the monitor it mostly covers. A centre in a gap or off the virtual desktop
/// returns `None`, letting callers decide whether to fall back (re-fit) or keep the old anchor
/// (follow loop).
pub fn monitor_for_window(monitors: &[MonitorInfo], window: WindowRect) -> Option<MonitorInfo> {
    let half_width = i32::try_from(window.size.0 / 2).unwrap_or(i32::MAX);
    let half_height = i32::try_from(window.size.1 / 2).unwrap_or(i32::MAX);
    let center = (
        window.position.0.saturating_add(half_width),
        window.position.1.saturating_add(half_height),
    );
    geometry::monitor_at_point(monitors, center).cloned()
}

/// Pure target selection for a sync: the window's monitor for [`GeometryTarget::CurrentPanel`],
/// the cursor's monitor for [`GeometryTarget::Cursor`].
///
/// Both paths fall back to the primary (then the first enumerated monitor) when their preferred
/// source is unavailable, so the reply always names a monitor when the desktop has one.
pub fn select_target(
    mode: GeometryTarget,
    monitors: &[MonitorInfo],
    cursor: Option<(i32, i32)>,
    panel: Option<WindowRect>,
) -> Option<MonitorInfo> {
    match mode {
        GeometryTarget::Cursor => target_monitor_at(monitors, cursor),
        GeometryTarget::CurrentPanel => panel
            .and_then(|window| monitor_for_window(monitors, window))
            .or_else(|| target_monitor_at(monitors, None)),
    }
}

/// One pointer-follow decision: the monitor the panel should move to, or `None`.
///
/// `busy` is `is_busy() || dragout::is_dragging()`: while a press/menu/native drag owns the mouse
/// the window must not move (a drag-out that crosses monitors would yank the source panel mid-OLE).
/// The pending candidate is dropped, so a full [`FOLLOW_DEBOUNCE_MS`] dwell is required after the
/// gesture ends; the caller re-evaluates on the next poll.
pub fn follow_target(
    follow: &mut MonitorFollow,
    observed: Option<&str>,
    now: u64,
    busy: bool,
) -> Option<String> {
    if busy {
        follow.reset_pending();
        return None;
    }
    follow.observe(observed, now, FOLLOW_DEBOUNCE_MS)
}

/// Stable signature of the display layout: ids, rectangles, work areas and DPI scales.
///
/// The reveal controller compares this every [`FOLLOW_POLL_MS`] while the panel is visible; a
/// changed signature (hotplug, resolution or DPI change) re-applies the window geometry even if the
/// pointer did not move. Formatting is deterministic for the sorted [`geometry::monitors`] list.
pub fn monitor_signature(monitors: &[MonitorInfo]) -> String {
    monitors
        .iter()
        .map(|monitor| {
            format!(
                "{}:{},{},{},{}:{},{},{},{}:{:.4}",
                monitor.id,
                monitor.rect.x,
                monitor.rect.y,
                monitor.rect.w,
                monitor.rect.h,
                monitor.work_area.x,
                monitor.work_area.y,
                monitor.work_area.w,
                monitor.work_area.h,
                monitor.scale_factor
            )
        })
        .collect::<Vec<_>>()
        .join(";")
}

/// Current `cardSize` setting (the panel height depends on it).
fn card_size(app: &AppHandle) -> CardSize {
    app.state::<AppState>().snapshot().settings.card_size
}

/// Physical position of the panel: the **work area** origin, never the full screen rect's.
///
/// The work area excludes taskbars/appbars, so a top or left taskbar moves the panel origin even
/// though the monitor's `rect` still starts at (0, 0) (design spec §3.3: the strip hugs the
/// usable desktop's top-left). [`apply_geometry`] and [`geometry_matches`] share this function so
/// the initial placement and the manual resync can never disagree.
pub fn panel_position(monitor: &MonitorInfo) -> PhysicalPosition<i32> {
    PhysicalPosition::new(monitor.work_area.x, monitor.work_area.y)
}

/// Payload of the `panel-geometry` event (P2-6): the panel window's fresh physical origin.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PanelGeometryEvent {
    /// Panel window physical origin x on the virtual screen.
    pub x: i32,
    /// Panel window physical origin y on the virtual screen.
    pub y: i32,
    /// Target monitor scale factor (informational; the frontend uses `devicePixelRatio` itself).
    pub scale: f64,
}

/// Moves and resizes the window onto `monitor`'s work area, returning the applied pixel size.
///
/// Position is applied before the size so the DPI context is the target monitor's; the size is
/// physical and therefore unaffected by tao's logical-size monitor resolution.
fn apply_geometry(
    win: &WebviewWindow,
    monitor: &MonitorInfo,
    size: CardSize,
) -> PhysicalSize<u32> {
    let position = panel_position(monitor);
    if let Err(err) = win.set_position(position) {
        log::warn!(
            "panel: failed to move to the work-area origin ({}, {}): {err}",
            position.x,
            position.y
        );
    }
    let pixels = panel_pixel_size(monitor, size);
    if let Err(err) = win.set_size(pixels) {
        log::warn!(
            "panel: failed to resize to {}x{} px: {err}",
            pixels.width,
            pixels.height
        );
    }
    pixels
}

/// Whether the window already has the target geometry (avoids redundant window API calls).
fn geometry_matches(
    win: &WebviewWindow,
    monitor: &MonitorInfo,
    size: CardSize,
) -> bool {
    let desired = panel_pixel_size(monitor, size);
    let target = panel_position(monitor);
    let (Ok(current), Ok(position)) = (win.outer_size(), win.outer_position()) else {
        return false;
    };
    current.width == desired.width
        && current.height == desired.height
        && position.x == target.x
        && position.y == target.y
}

/// Enumerates the monitors and re-applies the panel geometry to the requested target.
///
/// Task 14 contract: this is the only place that moves/resizes the panel window *after*
/// [`create`], which applies the initial geometry. The pure `get_monitors` query never moves
/// anything. Callers choose the target explicitly: the `sync_panel_geometry` command and
/// `set_setting` re-fit [`GeometryTarget::CurrentPanel`], while the reveal controller's debounced
/// pointer follow uses [`GeometryTarget::Cursor`] (review fix: a setting change must not teleport
/// the line to the pointer's monitor).
///
/// Returns the full monitor list, the chosen target and the geometry the window now has. A window
/// that already matches the target work area is left untouched (`moved: false`), which is what
/// makes the 200 ms poll cheap in the common case. When the window does not exist yet, `size` and
/// `position` stay `None`: the reply never claims geometry that was not applied.
pub fn sync_panel_geometry(app: &AppHandle, mode: GeometryTarget) -> PanelGeometry {
    let monitors = geometry::monitors();
    let win = app.get_webview_window(LABEL);
    let panel_rect = win.as_ref().and_then(window_rect);
    let cursor = matches!(mode, GeometryTarget::Cursor)
        .then(geometry::cursor_position)
        .flatten();
    let selected = select_target(mode, &monitors, cursor, panel_rect);
    let mut geometry = PanelGeometry {
        monitors,
        target: selected.clone(),
        size: None,
        position: None,
        moved: false,
    };
    let Some(target) = selected else {
        log::warn!("panel: no monitor enumerated; keeping the current geometry");
        return geometry;
    };

    let size_setting = card_size(app);
    let desired = panel_pixel_size(&target, size_setting);
    let desired_position = panel_position(&target);

    let Some(win) = win else {
        log::warn!(
            "panel: window is not open yet; not applying {mode:?} geometry to {}",
            target.id
        );
        return geometry;
    };
    if geometry_matches(&win, &target, size_setting) {
        geometry.size = Some(PixelSize {
            width: desired.width,
            height: desired.height,
        });
        geometry.position = Some(PixelPosition {
            x: desired_position.x,
            y: desired_position.y,
        });
        return geometry;
    }

    let pixels = apply_geometry(&win, &target, size_setting);
    // P2-6: the reveal controller moves the visible panel to follow the pointer across monitors;
    // the frontend needs the fresh physical origin to place fall overlays on the right monitor.
    let origin_event = PanelGeometryEvent {
        x: desired_position.x,
        y: desired_position.y,
        scale: valid_scale(&target),
    };
    if let Err(error) = app.emit("panel-geometry", origin_event) {
        log::warn!("panel: cannot emit panel-geometry: {error}");
    }
    geometry.size = Some(PixelSize {
        width: pixels.width,
        height: pixels.height,
    });
    geometry.position = Some(PixelPosition {
        x: desired_position.x,
        y: desired_position.y,
    });
    geometry.moved = true;
    log::info!(
        "panel: moved to {} at ({}, {}), {}x{} px (scale {:.2}, work area {}x{}, cardSize {:?})",
        target.id,
        target.work_area.x,
        target.work_area.y,
        pixels.width,
        pixels.height,
        valid_scale(&target),
        target.work_area.w,
        target.work_area.h,
        size_setting
    );
    geometry
}

/// Creates the panel window, initially hidden.
///
/// The window starts click-through; `hittest` turns interaction on while the cursor is
/// over a card. `focusable(false)` keeps the panel from ever becoming the foreground
/// window (design spec 4.2: the panel must never steal focus).
///
/// The panel is created hidden (design spec 4.2) and shown by the `report_panel_ready`
/// command once the frontend has painted its first layout — this avoids a white first-paint
/// flash when the transparent window appears.
pub fn create(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    let card_size = card_size(app);
    let monitors = geometry::monitors();
    let target = target_monitor(&monitors);
    // The builder's logical size is only the pre-show guess; `apply_geometry` below replaces it
    // with an exact physical size so mixed-DPI monitor resolution cannot distort the strip.
    let (width_dp, height_dp) = target
        .as_ref()
        .map(|monitor| {
            (
                f64::from(monitor.work_area.w.max(1)) / valid_scale(monitor),
                f64::from(panel_height(card_size)),
            )
        })
        .unwrap_or((FALLBACK_WIDTH_DP, FALLBACK_HEIGHT_DP));

    let mut builder = WebviewWindowBuilder::new(
        app,
        LABEL,
        WebviewUrl::App("index.html?window=panel".into()),
    )
    .title("Hangline")
    .inner_size(width_dp, height_dp)
    .position(0.0, 0.0)
    .transparent(true)
    .decorations(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .shadow(false)
    .resizable(false)
    .focused(false)
    .focusable(false)
    .visible(false);
    // Debug-only runtime-evidence hook; no-op in release builds (see `devtools`).
    if let Some(args) = crate::devtools::cdp_browser_args() {
        builder = builder.additional_browser_args(&args);
    }
    let win = builder.build()?;

    // Geometry is applied after creation: `position` + physical `set_size` are exact regardless
    // of which monitor tao picked for the builder's logical inner size.
    if let Some(monitor) = &target {
        let pixels = apply_geometry(&win, monitor, card_size);
        log::info!(
            "panel: {}x{} px at ({}, {}) on {} (scale {:.2}, work area {}x{}, cardSize {:?})",
            pixels.width,
            pixels.height,
            monitor.work_area.x,
            monitor.work_area.y,
            monitor.id,
            valid_scale(monitor),
            monitor.work_area.w,
            monitor.work_area.h,
            card_size
        );
    } else {
        log::warn!(
            "panel: no monitor enumerated; keeping the fallback {FALLBACK_WIDTH_DP}x{FALLBACK_HEIGHT_DP} dp strip"
        );
    }

    // Clicks pass through until the frontend reports a card rect under the cursor.
    win.set_ignore_cursor_events(true).ok();
    VISIBLE.store(false, Ordering::Relaxed);
    Ok(win)
}

/// Shows the panel (no-op before the window exists, and when it is already visible).
///
/// The visibility guard matters for the reveal controller: `panel::show` is called both on the
/// ready handshake and on every reveal, and re-showing an already visible window would re-run the
/// tray label sync and the window API for nothing.
pub fn show(app: &AppHandle) {
    if is_visible() {
        return;
    }
    let Some(win) = app.get_webview_window(LABEL) else {
        return;
    };
    match win.show() {
        Ok(()) => {
            VISIBLE.store(true, Ordering::Relaxed);
            log::debug!("panel: shown");
            // Keep the tray's show/hide label in sync with every visibility change.
            tray::sync_state(app);
        }
        Err(err) => log::warn!("panel: show failed: {err}"),
    }
}

/// Hides the panel; the hit-test thread stops polling while it is hidden.
pub fn hide(app: &AppHandle) {
    let Some(win) = app.get_webview_window(LABEL) else {
        return;
    };
    // Restore click-through before hiding so a later `show` can never intercept clicks in
    // transparent areas while the hit-test loop is still waking up from its idle sleep.
    if let Err(err) = win.set_ignore_cursor_events(true) {
        log::warn!("panel: failed to restore click-through before hiding: {err}");
    }
    match win.hide() {
        Ok(()) => {
            VISIBLE.store(false, Ordering::Relaxed);
            log::debug!("panel: hidden");
            tray::sync_state(app);
        }
        Err(err) => log::warn!("panel: hide failed: {err}"),
    }
}

/// Returns the panel visibility maintained by [`show`]/[`hide`].
///
/// This is the single visibility source for the hit-test gate; while it is `false` the
/// loop sleeps without touching the window at all.
pub fn is_visible() -> bool {
    VISIBLE.load(Ordering::Relaxed)
}

/// Shows the panel when hidden, hides it when visible.
pub fn toggle(app: &AppHandle) {
    if is_visible() {
        hide(app);
    } else {
        show(app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::PhysicalRect;

    /// A synthetic monitor whose work area is all the layout code needs.
    fn monitor(work_area: PhysicalRect, scale_factor: f64) -> MonitorInfo {
        monitor_with(work_area, work_area, scale_factor)
    }

    /// A synthetic monitor whose full screen `rect` differs from its `work_area` (taskbars).
    fn monitor_with(
        rect: PhysicalRect,
        work_area: PhysicalRect,
        scale_factor: f64,
    ) -> MonitorInfo {
        MonitorInfo {
            id: r"\\.\DISPLAY1".to_string(),
            rect,
            work_area,
            scale_factor,
            is_primary: true,
        }
    }

    #[test]
    fn panel_heights_match_the_spec_per_size() {
        assert_eq!(photo_max_height(CardSize::Small), 83);
        assert_eq!(photo_max_height(CardSize::Medium), 104);
        assert_eq!(photo_max_height(CardSize::Large), 140);
        assert_eq!(panel_height(CardSize::Small), 189);
        assert_eq!(panel_height(CardSize::Medium), 210);
        assert_eq!(panel_height(CardSize::Large), 246);
    }

    #[test]
    fn pixel_size_spans_the_whole_work_area_and_scales_the_height() {
        // 100%: the work area is already the exact physical width.
        let normal = monitor(
            PhysicalRect {
                x: 0,
                y: 0,
                w: 1920,
                h: 1032,
            },
            1.0,
        );
        assert_eq!(
            panel_pixel_size(&normal, CardSize::Medium),
            PhysicalSize::new(1920, 210)
        );

        // 150%: the same 1920 px work area stays fully covered, the 210 dp height becomes 315 px.
        let hidpi = monitor(
            PhysicalRect {
                x: -1920,
                y: 0,
                w: 1920,
                h: 1008,
            },
            1.5,
        );
        assert_eq!(
            panel_pixel_size(&hidpi, CardSize::Medium),
            PhysicalSize::new(1920, 315)
        );
        assert_eq!(
            panel_pixel_size(&hidpi, CardSize::Large),
            PhysicalSize::new(1920, 369)
        );

        // 125% rounds 189*1.25 = 236.25 -> 236 px (half away from zero).
        let mixed = monitor(
            PhysicalRect {
                x: 0,
                y: 0,
                w: 1280,
                h: 720,
            },
            1.25,
        );
        assert_eq!(
            panel_pixel_size(&mixed, CardSize::Small),
            PhysicalSize::new(1280, 236)
        );
    }

    #[test]
    fn panel_geometry_uses_work_area_not_the_full_screen_rect() {
        // Carry-over A guard: `rect` is the full screen, `work_area` excludes taskbars. The panel
        // must derive BOTH its physical size and its origin from `work_area`.

        // Bottom taskbar: same origin and width, shorter work area.
        let bottom_taskbar = monitor_with(
            PhysicalRect {
                x: 0,
                y: 0,
                w: 1920,
                h: 1080,
            },
            PhysicalRect {
                x: 0,
                y: 0,
                w: 1920,
                h: 1032,
            },
            1.0,
        );
        assert_eq!(
            panel_pixel_size(&bottom_taskbar, CardSize::Medium),
            PhysicalSize::new(1920, 210),
            "the height comes from the card size, not the work-area height"
        );
        assert_eq!(panel_position(&bottom_taskbar), PhysicalPosition::new(0, 0));

        // Left taskbar: the work area starts at x=80 and is 1840 wide; using `rect` would give a
        // 1920 px strip at x=0 that covers the taskbar and overhangs the desktop's right edge.
        let left_taskbar = monitor_with(
            PhysicalRect {
                x: 0,
                y: 0,
                w: 1920,
                h: 1080,
            },
            PhysicalRect {
                x: 80,
                y: 0,
                w: 1840,
                h: 1080,
            },
            1.0,
        );
        assert_eq!(
            panel_pixel_size(&left_taskbar, CardSize::Medium),
            PhysicalSize::new(1840, 210)
        );
        assert_eq!(panel_position(&left_taskbar), PhysicalPosition::new(80, 0));

        // Top taskbar: the panel origin follows the work-area y, not the screen's 0.
        let top_taskbar = monitor_with(
            PhysicalRect {
                x: 0,
                y: 0,
                w: 1920,
                h: 1080,
            },
            PhysicalRect {
                x: 0,
                y: 40,
                w: 1920,
                h: 1040,
            },
            1.0,
        );
        assert_eq!(panel_position(&top_taskbar), PhysicalPosition::new(0, 40));
        assert_eq!(
            panel_pixel_size(&top_taskbar, CardSize::Medium),
            PhysicalSize::new(1920, 210)
        );

        // Scaled monitor with a left taskbar: physical width is the work-area width (not the
        // 3840 px screen rect), and the height still scales by the monitor DPI.
        let scaled_left_taskbar = monitor_with(
            PhysicalRect {
                x: -3840,
                y: 0,
                w: 3840,
                h: 2160,
            },
            PhysicalRect {
                x: -3840,
                y: 0,
                w: 3680,
                h: 2160,
            },
            2.0,
        );
        assert_eq!(
            panel_pixel_size(&scaled_left_taskbar, CardSize::Medium),
            PhysicalSize::new(3680, 420)
        );
        assert_eq!(
            panel_position(&scaled_left_taskbar),
            PhysicalPosition::new(-3840, 0)
        );
    }

    #[test]
    fn pixel_size_degrades_gracefully_for_a_degenerate_monitor() {
        let degenerate = monitor(
            PhysicalRect {
                x: 0,
                y: 0,
                w: 0,
                h: 0,
            },
            0.0,
        );
        let size = panel_pixel_size(&degenerate, CardSize::Medium);
        assert_eq!(size.width, 1, "a window cannot be zero pixels wide");
        assert_eq!(size.height, 210, "an invalid scale falls back to 100%");
    }

    /// Task 14: 1920 px @ 100% and 1280 logical @ 150% are the same *logical* 1920x210 strip in
    /// different physical pixels; the webview must see `210 dp` in both cases so the capacity math
    /// and card heights match the design spec.
    #[test]
    fn pixel_size_keeps_the_logical_panel_size_across_dpi() {
        let normal = monitor(
            PhysicalRect {
                x: 0,
                y: 0,
                w: 1920,
                h: 1080,
            },
            1.0,
        );
        let normal_pixels = panel_pixel_size(&normal, CardSize::Medium);
        assert_eq!(normal_pixels, PhysicalSize::new(1920, 210));
        assert_eq!(f64::from(normal_pixels.width) / 1.0, 1920.0);
        assert_eq!(f64::from(normal_pixels.height) / 1.0, 210.0);

        // 1280 logical dp at 150% is 1920 physical px: the panel covers the same physical work
        // area, the height is 210 dp (= 315 px), and the webview inner width is 1280 dp.
        let hidpi = monitor(
            PhysicalRect {
                x: -1920,
                y: 0,
                w: 1920,
                h: 1080,
            },
            1.5,
        );
        let hidpi_pixels = panel_pixel_size(&hidpi, CardSize::Medium);
        assert_eq!(hidpi_pixels, PhysicalSize::new(1920, 315));
        assert_eq!(f64::from(hidpi_pixels.width) / 1.5, 1280.0);
        assert_eq!(f64::from(hidpi_pixels.height) / 1.5, 210.0);
        assert_eq!(
            panel_pixel_size(&hidpi, CardSize::Large),
            PhysicalSize::new(1920, 369)
        );
        // 189 dp * 1.5 = 283.5 px rounds away from zero to 284 px (189.33 dp in the webview); the
        // 0.33 dp difference is the unavoidable device-pixel grid, not a layout error.
        assert_eq!(
            panel_pixel_size(&hidpi, CardSize::Small),
            PhysicalSize::new(1920, 284)
        );
    }

    /// P4-3: placement contract across every work-area shape this build must survive (M2/M3).
    ///
    /// For each case the panel origin must be the target monitor's **work-area** top-left, the
    /// physical width must be that work area's width, and the physical height must be
    /// `panel_height(card_size) * scale` rounded to the nearest device pixel. Top taskbars
    /// (`work_area.y > 0`), left taskbars (`work_area.x > 0`), negative monitor origins and
    /// 125/150% scales are all exercised together, so a later change that derives geometry from
    /// `rect`, forgets the taskbar inset or truncates the scale conversion fails here.
    #[test]
    fn panel_origin_and_pixel_size_follow_the_work_area_across_taskbars_and_dpi() {
        struct Case {
            name: &'static str,
            rect: PhysicalRect,
            work_area: PhysicalRect,
            scale: f64,
            card: CardSize,
            expected: (u32, u32),
        }
        let cases = [
            Case {
                name: "primary, bottom taskbar, 100%",
                rect: PhysicalRect { x: 0, y: 0, w: 1920, h: 1080 },
                work_area: PhysicalRect { x: 0, y: 0, w: 1920, h: 1032 },
                scale: 1.0,
                card: CardSize::Medium,
                expected: (1920, 210),
            },
            Case {
                name: "primary, top taskbar, 100%",
                rect: PhysicalRect { x: 0, y: 0, w: 1920, h: 1080 },
                work_area: PhysicalRect { x: 0, y: 48, w: 1920, h: 1032 },
                scale: 1.0,
                card: CardSize::Medium,
                expected: (1920, 210),
            },
            Case {
                name: "primary, left taskbar, 100%",
                rect: PhysicalRect { x: 0, y: 0, w: 1920, h: 1080 },
                work_area: PhysicalRect { x: 80, y: 0, w: 1840, h: 1080 },
                scale: 1.0,
                card: CardSize::Medium,
                expected: (1840, 210),
            },
            Case {
                name: "secondary left of primary, bottom taskbar, 150%",
                rect: PhysicalRect { x: -1920, y: 0, w: 1920, h: 1080 },
                work_area: PhysicalRect { x: -1920, y: 0, w: 1920, h: 1008 },
                scale: 1.5,
                card: CardSize::Medium,
                expected: (1920, 315),
            },
            Case {
                name: "secondary left of primary, top taskbar, 150%",
                rect: PhysicalRect { x: -1920, y: 0, w: 1920, h: 1080 },
                work_area: PhysicalRect { x: -1920, y: 72, w: 1920, h: 1008 },
                scale: 1.5,
                card: CardSize::Medium,
                expected: (1920, 315),
            },
            Case {
                name: "secondary left of primary, left taskbar, 150%",
                rect: PhysicalRect { x: -1920, y: 0, w: 1920, h: 1080 },
                work_area: PhysicalRect { x: -1848, y: 0, w: 1848, h: 1080 },
                scale: 1.5,
                card: CardSize::Small,
                expected: (1848, 284),
            },
            Case {
                name: "monitor above the primary, top taskbar, 125%",
                rect: PhysicalRect { x: -1920, y: -1080, w: 1920, h: 1080 },
                work_area: PhysicalRect { x: -1920, y: -1032, w: 1920, h: 1032 },
                scale: 1.25,
                card: CardSize::Large,
                expected: (1920, 308),
            },
        ];

        for case in cases {
            let monitor = MonitorInfo {
                id: case.name.to_string(),
                rect: case.rect,
                work_area: case.work_area,
                scale_factor: case.scale,
                is_primary: false,
            };
            let position = panel_position(&monitor);
            assert_eq!(
                (position.x, position.y),
                (case.work_area.x, case.work_area.y),
                "{}: the panel origin must be the work-area top-left, not the screen rect's",
                case.name
            );
            let pixels = panel_pixel_size(&monitor, case.card);
            assert_eq!(
                (pixels.width, pixels.height),
                case.expected,
                "{}: unexpected physical panel size",
                case.name
            );
            assert_eq!(
                pixels.width,
                u32::try_from(case.work_area.w).unwrap(),
                "{}: the physical width is the work-area width",
                case.name
            );
            assert_eq!(
                i64::from(pixels.height),
                i64::from(geometry::dp_to_px(
                    f64::from(panel_height(case.card)),
                    case.scale
                )),
                "{}: the physical height is the dp height times the monitor scale, rounded",
                case.name
            );
            // The rounded pixel height maps back to the spec's dp height within half a device pixel.
            let logical_height = f64::from(pixels.height) / case.scale;
            assert!(
                (logical_height - f64::from(panel_height(case.card))).abs() <= 0.5,
                "{}: physical height {:.2} px is not the spec's {} dp at {}x",
                case.name,
                logical_height,
                panel_height(case.card),
                case.scale
            );
        }
    }

    /// P4-3: a display change that removes the panel's monitor must migrate the line, not leave it
    /// targeting a monitor that no longer exists.
    ///
    /// The reveal loop notices the changed [`monitor_signature`] and re-runs `sync_panel_geometry`;
    /// the pure selection is `select_target(CurrentPanel, ...)`: the window's centre no longer falls
    /// on any monitor, so the primary fallback owns the migration. The cursor is also gone (it was
    /// on the removed display), which is the same primary fallback on the `Cursor` path.
    #[test]
    fn display_change_migrates_the_panel_when_its_monitor_disappears() {
        let before = vec![
            named(r"\\.\DISPLAY1", -1920, 0, 1920, 1080, 1.5, false),
            named(r"\\.\DISPLAY2", 0, 0, 1920, 1080, 1.0, true),
        ];
        // The panel sits on the 150% secondary: origin (-1920, 0), 1920x315 px.
        let window = Some(WindowRect {
            position: (-1920, 0),
            size: (1920, 315),
        });
        let on_secondary = select_target(
            GeometryTarget::CurrentPanel,
            &before,
            Some((-1000, 500)),
            window,
        )
        .expect("the secondary monitor exists before the change");
        assert_eq!(on_secondary.id, r"\\.\DISPLAY1");
        assert_eq!(
            panel_position(&on_secondary),
            PhysicalPosition::new(-1920, 0)
        );
        assert_eq!(
            panel_pixel_size(&on_secondary, CardSize::Medium),
            PhysicalSize::new(1920, 315)
        );

        // The secondary display is unplugged: only the primary remains.
        let after = vec![named(r"\\.\DISPLAY2", 0, 0, 1920, 1080, 1.0, true)];
        assert_ne!(
            monitor_signature(&before),
            monitor_signature(&after),
            "the reveal loop only re-syncs when the signature changes"
        );
        let mut follow = MonitorFollow::default();
        assert!(!follow.display_changed(&monitor_signature(&before)));
        assert!(follow.display_changed(&monitor_signature(&after)));

        // The window rectangle still claims the vanished monitor; its centre now lies on no
        // monitor, so the primary is the migration target.
        assert_eq!(
            monitor_for_window(&after, window.unwrap()),
            None,
            "a stale window rect must not resolve to a monitor that disappeared"
        );
        let migrated = select_target(
            GeometryTarget::CurrentPanel,
            &after,
            Some((-1000, 500)),
            window,
        )
        .expect("the primary is always a fallback");
        assert_eq!(migrated.id, r"\\.\DISPLAY2");
        assert_eq!(panel_position(&migrated), PhysicalPosition::new(0, 0));
        assert_eq!(
            panel_pixel_size(&migrated, CardSize::Medium),
            PhysicalSize::new(1920, 210)
        );

        // A cursor left in the vanished monitor's area (a gap in the new layout) also resolves to
        // the primary, so the reveal follow cannot keep aiming at the removed display.
        assert_eq!(
            target_monitor_at(&after, Some((-1000, 500))).map(|monitor| monitor.id),
            Some(r"\\.\DISPLAY2".to_string())
        );
    }

    /// A named monitor with a work area equal to its rect (the tests below only need ids/geometry).
    fn named(
        id: &str,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        scale_factor: f64,
        is_primary: bool,
    ) -> MonitorInfo {
        MonitorInfo {
            id: id.to_string(),
            rect: PhysicalRect { x, y, w, h },
            work_area: PhysicalRect { x, y, w, h },
            scale_factor,
            is_primary,
        }
    }

    #[test]
    fn target_monitor_prefers_the_cursor_then_primary_then_first() {
        let monitors = vec![
            named(r"\\.\DISPLAY2", -1920, 0, 1920, 1080, 1.5, false),
            named(r"\\.\DISPLAY1", 0, 0, 1920, 1080, 1.0, true),
        ];
        let id_at = |cursor: Option<(i32, i32)>| {
            target_monitor_at(&monitors, cursor).map(|monitor| monitor.id)
        };

        assert_eq!(id_at(Some((-1000, 500))), Some(r"\\.\DISPLAY2".to_string()));
        assert_eq!(id_at(Some((500, 500))), Some(r"\\.\DISPLAY1".to_string()));
        // A cursor in a gap/off-desktop, or no cursor at all, falls back to the primary.
        assert_eq!(id_at(Some((5000, 500))), Some(r"\\.\DISPLAY1".to_string()));
        assert_eq!(id_at(None), Some(r"\\.\DISPLAY1".to_string()));

        // No primary flag: the first enumerated monitor is the last-resort target.
        let no_primary = vec![
            named(r"\\.\DISPLAY2", -1920, 0, 1920, 1080, 1.5, false),
            named(r"\\.\DISPLAY1", 0, 0, 1920, 1080, 1.0, false),
        ];
        assert_eq!(
            target_monitor_at(&no_primary, None).map(|monitor| monitor.id),
            Some(r"\\.\DISPLAY2".to_string())
        );
        assert_eq!(target_monitor_at(&[], None), None);
    }

    #[test]
    fn top_edge_hit_is_per_monitor_and_scales_the_tolerance() {
        let monitors = vec![
            named(r"\\.\DISPLAY2", -1920, 0, 1920, 1080, 1.5, false),
            named(r"\\.\DISPLAY1", 0, 0, 1920, 1080, 1.0, true),
        ];
        let left = &monitors[0];
        let right = &monitors[1];
        // 150% monitor: 2 dp rounds to 3 px; 100% monitor: 2 dp is 2 px.
        assert!(top_edge_hit(left, (-1000, 0), 2.0));
        assert!(top_edge_hit(left, (-1000, 3), 2.0));
        assert!(!top_edge_hit(left, (-1000, 4), 2.0));
        assert!(top_edge_hit(right, (1000, 0), 2.0));
        assert!(top_edge_hit(right, (1000, 2), 2.0));
        assert!(!top_edge_hit(right, (1000, 3), 2.0));
        // A point on the other monitor never matches this monitor, even at the same y.
        assert!(!top_edge_hit(right, (-1000, 0), 2.0));
        assert!(!top_edge_hit(left, (1000, 0), 2.0));
        // Not on the top edge, or outside this monitor horizontally.
        assert!(!top_edge_hit(left, (-1000, 100), 2.0));
        assert!(!top_edge_hit(left, (-2000, 0), 2.0));
    }

    #[test]
    fn top_edge_hit_handles_a_monitor_above_the_primary() {
        let monitors = vec![
            named(r"\\.\DISPLAY1", 0, 0, 1920, 1080, 1.0, true),
            named(r"\\.\DISPLAY2", 0, -1080, 1920, 1080, 1.0, false),
        ];
        let target = |point| top_edge_target(&monitors, point, 2.0).map(|monitor| monitor.id.as_str());
        assert_eq!(target((900, -1080)), Some(r"\\.\DISPLAY2"));
        assert_eq!(target((900, 0)), Some(r"\\.\DISPLAY1"));
        // y=1 belongs to the primary's top edge; it is not inside the negative-y monitor.
        assert_eq!(target((900, 1)), Some(r"\\.\DISPLAY1"));
        assert_eq!(target((900, -1076)), None);
    }

    #[test]
    fn top_edge_target_looks_at_the_pointer_monitor_not_the_panels_monitor() {
        let monitors = vec![
            named(r"\\.\DISPLAY2", -1920, 0, 1920, 1080, 1.5, false),
            named(r"\\.\DISPLAY1", 0, 0, 1920, 1080, 1.0, true),
        ];
        let target = |point| top_edge_target(&monitors, point, 2.0).map(|monitor| monitor.id.as_str());
        assert_eq!(target((-1000, 1)), Some(r"\\.\DISPLAY2"));
        assert_eq!(target((1000, 1)), Some(r"\\.\DISPLAY1"));
        // Mid-screen is not an edge.
        assert_eq!(target((-1000, 500)), None);
        assert_eq!(target((1000, 500)), None);

        // A top taskbar moves the work area down; the edge is the work area's top, not y=0.
        let top_taskbar = vec![MonitorInfo {
            id: r"\\.\DISPLAY1".to_string(),
            rect: PhysicalRect {
                x: 0,
                y: 0,
                w: 1920,
                h: 1080,
            },
            work_area: PhysicalRect {
                x: 0,
                y: 40,
                w: 1920,
                h: 1040,
            },
            scale_factor: 1.0,
            is_primary: true,
        }];
        assert_eq!(top_edge_target(&top_taskbar, (1000, 0), 2.0), None);
        assert_eq!(
            top_edge_target(&top_taskbar, (1000, 41), 2.0).map(|monitor| monitor.id.as_str()),
            Some(r"\\.\DISPLAY1")
        );
    }

    #[test]
    fn monitor_signature_tracks_layout_and_dpi_changes() {
        let base = vec![
            named(r"\\.\DISPLAY2", -1920, 0, 1920, 1080, 1.5, false),
            named(r"\\.\DISPLAY1", 0, 0, 1920, 1080, 1.0, true),
        ];
        let signature = monitor_signature(&base);
        assert_eq!(signature, monitor_signature(&base.clone()));

        let mut resized = base.clone();
        resized[1].rect.w = 2560;
        resized[1].work_area.w = 2560;
        assert_ne!(signature, monitor_signature(&resized));

        let mut dpi_changed = base.clone();
        dpi_changed[0].scale_factor = 1.25;
        assert_ne!(signature, monitor_signature(&dpi_changed));

        let mut removed = base.clone();
        removed.remove(0);
        assert_ne!(signature, monitor_signature(&removed));
    }

    /// Moving the taskbar to the top leaves the monitor `rect` unchanged but moves the work area:
    /// the signature must change so the reveal loop re-applies the panel origin (M3).
    #[test]
    fn monitor_signature_tracks_a_taskbar_move_with_an_unchanged_screen_rect() {
        let bottom_taskbar = vec![MonitorInfo {
            id: r"\\.\DISPLAY1".to_string(),
            rect: PhysicalRect { x: 0, y: 0, w: 1920, h: 1080 },
            work_area: PhysicalRect { x: 0, y: 0, w: 1920, h: 1032 },
            scale_factor: 1.0,
            is_primary: true,
        }];
        let mut top_taskbar = bottom_taskbar.clone();
        top_taskbar[0].work_area = PhysicalRect {
            x: 0,
            y: 40,
            w: 1920,
            h: 1040,
        };
        // Same rect and DPI on purpose: only the taskbar position differs.
        assert_eq!(top_taskbar[0].rect, bottom_taskbar[0].rect);
        assert_ne!(
            monitor_signature(&bottom_taskbar),
            monitor_signature(&top_taskbar),
            "a moved taskbar changes the work area and must force a geometry re-sync"
        );
        // The origin follows the new work area (the panel is not left under the taskbar).
        assert_eq!(
            panel_position(&top_taskbar[0]),
            PhysicalPosition::new(0, 40)
        );
    }

    #[test]
    fn monitor_follow_adopts_the_startup_monitor_without_moving() {
        let mut follow = MonitorFollow::default();
        assert_eq!(follow.applied, None);
        assert_eq!(
            follow.observe(Some(r"\\.\DISPLAY1"), 0, FOLLOW_DEBOUNCE_MS),
            None,
            "the startup window was already created on the cursor's monitor"
        );
        assert_eq!(follow.applied.as_deref(), Some(r"\\.\DISPLAY1"));
        assert_eq!(
            follow.observe(Some(r"\\.\DISPLAY1"), 60_000, FOLLOW_DEBOUNCE_MS),
            None
        );
        // A follow target that already is the applied monitor is never re-applied.
        assert_eq!(
            follow.observe(Some(r"\\.\DISPLAY2"), 61_000, FOLLOW_DEBOUNCE_MS),
            None,
            "the first sample only starts the debounce"
        );
        assert_eq!(
            follow.observe(Some(r"\\.\DISPLAY2"), 61_500, FOLLOW_DEBOUNCE_MS),
            Some(r"\\.\DISPLAY2".to_string())
        );
        assert_eq!(follow.applied.as_deref(), Some(r"\\.\DISPLAY2"));
    }

    #[test]
    fn monitor_follow_ignores_a_brief_crossing_and_a_missing_cursor() {
        let mut follow = MonitorFollow::default();
        assert_eq!(follow.observe(Some("A"), 0, FOLLOW_DEBOUNCE_MS), None);
        assert_eq!(follow.observe(Some("B"), 100, FOLLOW_DEBOUNCE_MS), None);
        // Back on A before 500 ms: the candidate is dropped, no move.
        assert_eq!(follow.observe(Some("A"), 300, FOLLOW_DEBOUNCE_MS), None);
        assert_eq!(follow.observe(Some("B"), 400, FOLLOW_DEBOUNCE_MS), None);
        // A cursor gap (between monitors / query failure) restarts the debounce.
        assert_eq!(follow.observe(None, 700, FOLLOW_DEBOUNCE_MS), None);
        assert_eq!(follow.observe(Some("B"), 800, FOLLOW_DEBOUNCE_MS), None);
        assert_eq!(
            follow.observe(Some("B"), 1_299, FOLLOW_DEBOUNCE_MS),
            None
        );
        assert_eq!(
            follow.observe(Some("B"), 1_300, FOLLOW_DEBOUNCE_MS),
            Some("B".to_string())
        );
    }

    #[test]
    fn monitor_follow_can_be_reanchored_after_a_display_change() {
        let mut follow = MonitorFollow::default();
        assert_eq!(follow.observe(Some("A"), 0, FOLLOW_DEBOUNCE_MS), None);
        follow.set_applied("B");
        assert_eq!(follow.applied.as_deref(), Some("B"));
        assert_eq!(follow.observe(Some("B"), 10, FOLLOW_DEBOUNCE_MS), None);
        assert_eq!(
            follow.observe(Some("A"), 20, FOLLOW_DEBOUNCE_MS),
            None,
            "a monitor change after re-anchoring needs the full debounce again"
        );
        assert_eq!(
            follow.observe(Some("A"), 520, FOLLOW_DEBOUNCE_MS),
            Some("A".to_string())
        );
    }

    #[test]
    fn monitor_follow_reports_a_display_layout_change_once() {
        let mut follow = MonitorFollow::default();
        assert!(!follow.display_changed("one"), "the first sample only records");
        assert!(!follow.display_changed("one"));
        assert!(follow.display_changed("two"));
        assert!(!follow.display_changed("two"));
        assert!(follow.display_changed("two+extended"));
    }

    /// Review fix: a setting change re-fits the window's monitor, only the follow uses the cursor.
    #[test]
    fn sync_target_refits_the_current_panel_and_follows_the_cursor() {
        let monitors = vec![
            named(r"\\.\DISPLAY2", -1920, 0, 1920, 1080, 1.5, false),
            named(r"\\.\DISPLAY1", 0, 0, 1920, 1080, 1.0, true),
        ];
        let cursor_on_display2 = Some((-1000, 500));
        let window_on_display1 = Some(WindowRect {
            position: (0, 0),
            size: (1920, 210),
        });
        let id = |mode, cursor, window| {
            select_target(mode, &monitors, cursor, window).map(|monitor| monitor.id)
        };

        // The panel is visible on DISPLAY1 while the settings window (cursor) is on DISPLAY2.
        // A cardSize/keepOnLine change must stay on DISPLAY1 ...
        assert_eq!(
            id(
                GeometryTarget::CurrentPanel,
                cursor_on_display2,
                window_on_display1
            ),
            Some(r"\\.\DISPLAY1".to_string())
        );
        // ... while the reveal follow moves to the pointer's monitor.
        assert_eq!(
            id(GeometryTarget::Cursor, cursor_on_display2, window_on_display1),
            Some(r"\\.\DISPLAY2".to_string())
        );
        // No window yet (startup): the re-fit falls back to the primary, not the cursor.
        assert_eq!(
            id(GeometryTarget::CurrentPanel, cursor_on_display2, None),
            Some(r"\\.\DISPLAY1".to_string())
        );
        // A window whose centre is off every monitor uses the same primary fallback.
        let off_desktop = Some(WindowRect {
            position: (5000, 5000),
            size: (10, 10),
        });
        assert_eq!(
            id(GeometryTarget::CurrentPanel, None, off_desktop),
            Some(r"\\.\DISPLAY1".to_string())
        );
        // A cursor in a gap/off-desktop still resolves through the primary fallback.
        assert_eq!(
            id(GeometryTarget::Cursor, Some((5000, 500)), window_on_display1),
            Some(r"\\.\DISPLAY1".to_string())
        );
    }

    #[test]
    fn window_monitor_derivation_uses_the_window_centre() {
        let monitors = vec![
            named(r"\\.\DISPLAY2", -1920, 0, 1920, 1080, 1.5, false),
            named(r"\\.\DISPLAY1", 0, 0, 1920, 1080, 1.0, true),
        ];
        let at = |position, size| {
            monitor_for_window(&monitors, WindowRect { position, size })
                .map(|monitor| monitor.id)
        };

        assert_eq!(at((0, 0), (1920, 210)), Some(r"\\.\DISPLAY1".to_string()));
        assert_eq!(
            at((-1920, 0), (1920, 315)),
            Some(r"\\.\DISPLAY2".to_string())
        );
        // Centre, not origin: a 1920 px window at x=-1920 has its centre at -960 (DISPLAY2).
        assert_eq!(
            at((-1920, 0), (1920, 210)),
            Some(r"\\.\DISPLAY2".to_string())
        );
        // A centre that falls in a gap between monitors has no monitor.
        let gap = vec![
            named("left", 0, 0, 100, 100, 1.0, true),
            named("right", 200, 0, 100, 100, 1.0, false),
        ];
        assert_eq!(
            monitor_for_window(
                &gap,
                WindowRect {
                    position: (0, 0),
                    size: (300, 10),
                }
            ),
            None,
            "centre (150, 5) is in the 100..200 gap"
        );
    }

    /// Review fix: the follow anchor must come from the window's real monitor, not only from the
    /// loop's previous sync, or an external move masks the pointer-return path forever.
    #[test]
    fn monitor_follow_adopts_an_externally_moved_window() {
        let mut follow = MonitorFollow::default();
        assert_eq!(follow.observe(Some("A"), 0, FOLLOW_DEBOUNCE_MS), None);
        assert_eq!(follow.applied.as_deref(), Some("A"));

        // The window was moved to B by something else while the pointer never left A.
        follow.anchor_to_window(Some("B"));
        assert_eq!(follow.applied.as_deref(), Some("B"));

        // Returning to A is a real monitor change now: a normal dwell follows, then the move.
        assert_eq!(follow.observe(Some("A"), 100, FOLLOW_DEBOUNCE_MS), None);
        assert_eq!(
            follow.observe(Some("A"), 600, FOLLOW_DEBOUNCE_MS),
            Some("A".to_string())
        );

        // Anchoring to the monitor the window already has is a no-op.
        follow.anchor_to_window(Some("A"));
        assert_eq!(follow.observe(Some("A"), 700, FOLLOW_DEBOUNCE_MS), None);
        // A seam (no monitor contains the window centre) keeps the previous anchor.
        follow.anchor_to_window(None);
        assert_eq!(follow.applied.as_deref(), Some("A"));
    }

    /// Review fix: no follow move may happen while a press/menu/native drag owns the mouse.
    #[test]
    fn follow_target_never_moves_while_busy_and_dwells_again_afterward() {
        let mut follow = MonitorFollow::default();
        assert_eq!(follow_target(&mut follow, Some("A"), 0, false), None);

        // A drag-out that dwells on B for >500 ms must not accumulate a follow candidate.
        assert_eq!(follow_target(&mut follow, Some("B"), 100, true), None);
        assert_eq!(follow_target(&mut follow, Some("B"), 900, true), None);
        assert_eq!(follow.pending, None, "busy polls drop the candidate");

        // Once the gesture ends, B needs a full 500 ms dwell from the first post-gesture sample.
        assert_eq!(follow_target(&mut follow, Some("B"), 1_000, false), None);
        assert_eq!(follow_target(&mut follow, Some("B"), 1_499, false), None);
        assert_eq!(
            follow_target(&mut follow, Some("B"), 1_500, false),
            Some("B".to_string())
        );
    }
}

