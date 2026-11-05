//! The screen-wide fly-in / fall overlay (design spec §4.2 "fly", §4.4 events, §4.6 B/C).
//!
//! One transparent, click-through `fly` webview covers the whole virtual desktop. It is created
//! hidden at startup ("pre-warmed") and reused for every flight: creating a fresh WebView2 window
//! per flight costs hundreds of milliseconds and made the P1 fallback look like a plain card
//! appearance. The page inside owns the animation; Rust owns the window, the concurrency limit
//! (`MAX_ACTIVE`) and the animation payloads so a late subscriber can recover them.
//!
//! Flow of a fly-in:
//! 1. panel calls `start_fly(id, from?, to, tiltDeg)`;
//! 2. Rust stores the animation, shows the overlay and emits `fly-animation` with the full active
//!    list (≤2 animations);
//! 3. the overlay animates and calls `fly_done(id)` when the card landed;
//! 4. Rust removes it, emits `fly-landed { id }` to the panel and hides the window when idle.
//!
//! A missing `from` (system screenshot, clipboard, watched folder) is the spec §4.6 C fallback:
//! a 120×80 logical-dp rectangle centred on the monitor that contains the target, scaled by that
//! monitor's DPI. The panel's own card is hidden (`data-flying`) for the whole flight so the panel
//! does not paint a duplicate card behind the overlay; the overlay is always click-through, so the
//! user can keep clicking the frozen desktop underneath.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder,
};

use crate::app_state::AppState;
use crate::capture;
use crate::devtools;
use crate::geometry::{dp_to_px, monitors, MonitorInfo, PhysicalRect};

/// Webview label of the overlay window (design spec §4.2).
pub const WINDOW_LABEL: &str = "fly";

/// At most two animations on screen at once (design spec §4.2: "页内最多 2 个并发动画").
pub const MAX_ACTIVE: usize = 2;

/// Overlay event carrying the full active animation list (design spec §4.4).
const ANIMATION_EVENT: &str = "fly-animation";

/// Overlay event carrying a (re)computed virtual-screen rect + theme, e.g. after a display change.
const GEOMETRY_EVENT: &str = "fly-geometry";

/// Panel event: one fly animation finished, the card may become visible (design spec §4.4).
const LANDED_EVENT: &str = "fly-landed";

/// Fallback start size in logical dp for items without a capture origin (spec §4.6 C).
const FALLBACK_W_DP: f64 = 120.0;
const FALLBACK_H_DP: f64 = 80.0;

/// Fly-in duration in ms (spec §4.6 B: 0.65 s `easeInOutCubic`).
const FLY_DURATION_MS: u32 = 650;

/// Fall duration in ms (spec §4.6 B: 0.55 s, `k³` ease-in).
const FALL_DURATION_MS: u32 = 550;

/// Cross-fade after `fly_done`: the overlay stays visible while the panel card fades in underneath
/// (reference: completion first, then the 0.16 s fade). The overlay must not hide immediately.
const HANDOFF_FADE_MS: u64 = 160;

/// A missing `fly_done` (renderer crash/HMR) is cleaned up after duration + this margin.
const WATCHDOG_MARGIN_MS: u64 = 1000;

/// Which overlay animation to play (spec §4.4: `kind = fly | fall`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FlyKind {
    /// A new card flies from the capture area (or fallback origin) onto the line.
    Fly,
    /// A taken-down card falls off the line (P2-6 owns the panel trigger; the renderer is here).
    Fall,
}

impl FlyKind {
    /// Animation duration in milliseconds for this kind.
    fn duration_ms(self) -> u32 {
        match self {
            FlyKind::Fly => FLY_DURATION_MS,
            FlyKind::Fall => FALL_DURATION_MS,
        }
    }

    /// Log tag for the runtime-evidence lines.
    fn as_str(self) -> &'static str {
        match self {
            FlyKind::Fly => "fly",
            FlyKind::Fall => "fall",
        }
    }
}

/// One active overlay animation (serialized camelCase for the TypeScript `FlyAnimation`).
///
/// `from`/`to` are virtual-desktop physical pixels; `tilt_deg` is the card's deterministic panel
/// tilt (the overlay interpolates towards it); `duration_ms` comes from [`FlyKind`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlyAnimation {
    /// Opaque item id (the panel's `fly-landed` payload and the overlay's `fly_done` key).
    pub id: String,
    /// Which animation to play.
    pub kind: FlyKind,
    /// Absolute path of the original image the overlay renders.
    pub image_path: PathBuf,
    /// Last rewrite time of that file (`Item.updated_at`), used as the overlay's `?v=` cache key.
    ///
    /// P3: a markup save rewrites the file in place and the panel switches to a versioned URL; the
    /// overlay must load the same version or a WebView byte cache could show the pre-edit image.
    pub image_version: i64,
    /// Start rectangle on the virtual screen (physical px).
    pub from: PhysicalRect,
    /// End rectangle on the virtual screen (physical px).
    pub to: PhysicalRect,
    /// Final card tilt in degrees (panel `tiltFor` value).
    pub tilt_deg: f64,
    /// Animation length in ms.
    pub duration_ms: u32,
    /// DPI scale of the monitor the card lands on (`1.0` = 100 %, `1.5` = 150 %).
    ///
    /// Physical positions/sizes are already exact; the overlay page turns this into its chrome
    /// scale (`target_scale / devicePixelRatio`) so dp values (arc/inset/radius/pin/fall distance)
    /// stay physically correct on mixed-DPI desktops without trusting the multi-monitor window's
    /// own scale factor.
    pub target_scale: f64,
}

/// Payload of the `fly-animation` event.
#[derive(Clone, Debug, Serialize)]
struct AnimationsPayload {
    animations: Vec<FlyAnimation>,
}

/// Payload of the `fly-geometry` event / return value of `report_fly_ready`.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GeometryPayload {
    virtual_rect: PhysicalRect,
    /// Raw `settings.theme` (`system`/`light`/`dark`); the page resolves `system` itself.
    theme: String,
}

/// Payload of the `fly-landed` event.
#[derive(Clone, Debug, Serialize)]
struct LandedPayload {
    id: String,
}

/// The overlay window + animation list, both behind [`FlyManager`].
#[derive(Default)]
struct FlyState {
    /// The pre-warmed hidden overlay window, created in `setup`.
    window: Option<WebviewWindow>,
    /// True once the fly page registered its listener and reported ready.
    ready: bool,
    /// The virtual-screen rectangle the overlay covers (physical px), if known.
    virtual_rect: Option<PhysicalRect>,
    /// Active animations keyed by item id (`BTreeMap` → deterministic payload order for tests/logs).
    active: BTreeMap<String, FlyAnimation>,
}

/// Managed state for the fly overlay (registered in `lib.rs`).
#[derive(Default)]
pub struct FlyManager(Mutex<FlyState>);

impl FlyManager {
    /// Locks the state; a poisoned lock only means a previous overlay panic, so recover the data.
    fn lock(&self) -> std::sync::MutexGuard<'_, FlyState> {
        self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Whether a new animation may start: the page must be ready and the cap not reached.
fn can_start(ready: bool, active_len: usize) -> bool {
    ready && active_len < MAX_ACTIVE
}

/// The full active animation list in deterministic order.
fn animation_list(active: &BTreeMap<String, FlyAnimation>) -> Vec<FlyAnimation> {
    active.values().cloned().collect()
}

/// Spec §4.6 C fallback: a 120×80 dp rectangle centred on the monitor containing `to`.
///
/// The rectangle is scaled by the monitor's DPI (mixed-DPI desktops stay physical-pixel correct).
/// When the monitor list is empty (unexpected: EnumDisplayMonitors always finds the primary) the
/// fallback is centred on the target itself at 100%, so the overlay still renders something sane.
fn fallback_from_rect(to: PhysicalRect, monitors: &[MonitorInfo]) -> PhysicalRect {
    let center_x = to.x.saturating_add(to.w / 2);
    let center_y = to.y.saturating_add(to.h / 2);
    let monitor = monitors
        .iter()
        .find(|monitor| monitor.rect.contains((center_x, center_y)))
        .or_else(|| monitors.iter().find(|monitor| monitor.is_primary))
        .or_else(|| monitors.first());
    let Some(monitor) = monitor else {
        let w = (FALLBACK_W_DP as i32).max(1);
        let h = (FALLBACK_H_DP as i32).max(1);
        return PhysicalRect {
            x: center_x - w / 2,
            y: center_y - h / 2,
            w,
            h,
        };
    };
    let w = dp_to_px(FALLBACK_W_DP, monitor.scale_factor).max(1);
    let h = dp_to_px(FALLBACK_H_DP, monitor.scale_factor).max(1);
    PhysicalRect {
        x: monitor.rect.x + (monitor.rect.w - w) / 2,
        y: monitor.rect.y + (monitor.rect.h - h) / 2,
        w,
        h,
    }
}

/// Creates the transparent, click-through overlay window once and returns a clone.
///
/// The window is built hidden; the caller shows it when the first animation starts. Position/size
/// are set in **physical** pixels after creation so negative-origin and mixed-DPI monitors are
/// exact (the builder's logical arguments are not used for geometry).
fn ensure_window(app: &AppHandle) -> anyhow::Result<WebviewWindow> {
    if let Some(window) = {
        let state = app
            .try_state::<FlyManager>()
            .ok_or_else(|| anyhow::anyhow!("fly: the overlay state is not managed"))?;
        let existing = state.lock().window.clone();
        existing
    } {
        return Ok(window);
    }
    let screen = capture::virtual_screen_rect();
    let mut builder = WebviewWindowBuilder::new(
        app,
        WINDOW_LABEL,
        WebviewUrl::App("index.html?window=fly".into()),
    )
    .title("Hangline fly")
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
    if let Some(args) = devtools::cdp_browser_args() {
        builder = builder.additional_browser_args(&args);
    }
    let window = builder.build()?;
    window.set_position(PhysicalPosition::new(screen.x, screen.y))?;
    window.set_size(PhysicalSize::new(
        screen.w.max(1) as u32,
        screen.h.max(1) as u32,
    ))?;
    window.set_ignore_cursor_events(true)?;
    let state = app
        .try_state::<FlyManager>()
        .ok_or_else(|| anyhow::anyhow!("fly: the overlay state is not managed"))?;
    let mut guard = state.lock();
    guard.window = Some(window.clone());
    guard.virtual_rect = Some(screen);
    log::info!(
        "fly: overlay window created at ({}, {}) {}x{} px",
        screen.x,
        screen.y,
        screen.w,
        screen.h
    );
    Ok(window)
}

/// Creates the hidden overlay at startup so the first flight does not pay WebView2 cold start.
pub fn prewarm(app: &AppHandle) -> anyhow::Result<()> {
    ensure_window(app)?;
    Ok(())
}

/// Resolved theme tag for the overlay page (`system` stays unresolved; the page follows the OS).
fn theme_name(app: &AppHandle) -> String {
    let theme = app
        .try_state::<AppState>()
        .map(|state| state.snapshot().settings.theme)
        .unwrap_or_default();
    // One wire-name source for every window (`Theme::as_str`, P2-8 review).
    theme.as_str().to_string()
}

/// DPI scale of the monitor containing `to` (`1.0` when it cannot be determined).
///
/// The overlay page divides this by its own `devicePixelRatio` to derive `chromeScale`: the
/// fly window spans multiple monitors, so its own scale factor is ambiguous, while the page's DPR
/// is exactly the CSS↔physical ratio of the surface it renders on.
fn target_scale_for(to: PhysicalRect, monitors: &[MonitorInfo]) -> f64 {
    let center = (
        to.x.saturating_add(to.w / 2),
        to.y.saturating_add(to.h / 2),
    );
    monitors
        .iter()
        .find(|monitor| monitor.rect.contains(center))
        .or_else(|| monitors.iter().find(|monitor| monitor.is_primary))
        .or_else(|| monitors.first())
        .map(|monitor| monitor.scale_factor)
        .filter(|scale| *scale > 0.0 && scale.is_finite())
        .unwrap_or(1.0)
}

/// Re-reads the virtual-screen rect and resizes/repositions the overlay when a display changed.
///
/// Returns `true` when the geometry was updated (the caller then emits [`GEOMETRY_EVENT`] so the
/// page recomputes its physical→CSS scale and theme).
fn sync_geometry(app: &AppHandle, window: &WebviewWindow) -> anyhow::Result<bool> {
    let screen = capture::virtual_screen_rect();
    let changed = {
        let state = app
            .try_state::<FlyManager>()
            .ok_or_else(|| anyhow::anyhow!("fly: the overlay state is not managed"))?;
        let mut guard = state.lock();
        if guard.virtual_rect == Some(screen) {
            false
        } else {
            guard.virtual_rect = Some(screen);
            true
        }
    };
    if changed {
        window.set_position(PhysicalPosition::new(screen.x, screen.y))?;
        window.set_size(PhysicalSize::new(
            screen.w.max(1) as u32,
            screen.h.max(1) as u32,
        ))?;
        log::info!(
            "fly: virtual screen changed; overlay resized to ({}, {}) {}x{}",
            screen.x,
            screen.y,
            screen.w,
            screen.h
        );
    }
    Ok(changed)
}

/// The monitor list from Tauri (tao) when possible, falling back to the raw Win32 enumeration.
///
/// `geometry::monitors()` assumes a Per-Monitor-V2 thread; Tauri v2 runs synchronous commands on
/// its IPC thread, which is not guaranteed to be PMV2 yet, so `GetDpiForMonitor` can be virtualized
/// to 100%. Tauri's own monitor list comes from the windowing system and carries the real scale
/// factors, so it is authoritative for the overlay geometry (mixed-DPI flight targets).
fn runtime_monitors(app: &AppHandle) -> Vec<MonitorInfo> {
    let primary = app
        .primary_monitor()
        .ok()
        .flatten()
        .map(|monitor| (monitor.position().x, monitor.position().y));
    match app.available_monitors() {
        Ok(monitors) if !monitors.is_empty() => monitors
            .iter()
            .enumerate()
            .map(|(index, monitor)| {
                let position = monitor.position();
                let size = monitor.size();
                let rect = PhysicalRect {
                    x: position.x,
                    y: position.y,
                    w: size.width as i32,
                    h: size.height as i32,
                };
                MonitorInfo {
                    id: monitor
                        .name()
                        .cloned()
                        .unwrap_or_else(|| format!("monitor-{index}")),
                    rect,
                    work_area: rect,
                    scale_factor: monitor.scale_factor(),
                    is_primary: primary == Some((position.x, position.y)),
                }
            })
            .collect(),
        _ => monitors(),
    }
}

/// Shows the overlay and emits the current animation list.
///
/// Click-through is re-asserted before showing and is a hard requirement: a full-screen
/// always-on-top window that is *not* click-through would swallow every click on the desktop. On
/// failure the window is hidden again and the caller reports the animation as rejected.
fn present(app: &AppHandle, window: &WebviewWindow) -> anyhow::Result<()> {
    if let Err(error) = window.set_ignore_cursor_events(true) {
        let _ = window.hide();
        anyhow::bail!("fly: cannot keep the overlay click-through: {error}");
    }
    if let Err(error) = window.show() {
        let _ = window.hide();
        anyhow::bail!("fly: cannot show the overlay: {error}");
    }
    let payload = {
        let Some(state) = app.try_state::<FlyManager>() else {
            return Ok(());
        };
        let guard = state.lock();
        AnimationsPayload {
            animations: animation_list(&guard.active),
        }
    };
    if let Err(error) = app.emit_to(WINDOW_LABEL, ANIMATION_EVENT, payload) {
        log::warn!("fly: cannot emit {ANIMATION_EVENT}: {error}");
    }
    Ok(())
}

/// Hides the overlay when no animation is active any more (idempotent).
fn hide_if_idle(app: &AppHandle) {
    let Some(state) = app.try_state::<FlyManager>() else {
        return;
    };
    let window = {
        let guard = state.lock();
        if !guard.active.is_empty() {
            return;
        }
        guard.window.clone()
    };
    let Some(window) = window else {
        return;
    };
    if let Err(error) = window.hide() {
        log::warn!("fly: cannot hide the idle overlay: {error}");
        return;
    }
    log::info!("fly: overlay hidden (idle)");
    // A new flight may have been inserted between the emptiness check and `hide()` (a concurrent
    // IPC command). Re-check and undo the hide instead of leaving an invisible-but-"shown" overlay.
    let animations = {
        let guard = state.lock();
        if guard.active.is_empty() {
            return;
        }
        animation_list(&guard.active)
    };
    if let Err(error) = window.set_ignore_cursor_events(true) {
        // Fail closed: never show a full-screen always-on-top window that is not click-through.
        log::warn!(
            "fly: cannot re-arm click-through after a raced idle hide; keeping the overlay hidden: \
             {error}"
        );
        return;
    }
    if let Err(error) = window.show() {
        log::warn!("fly: cannot re-show the overlay after a raced idle hide: {error}");
    }
    if let Err(error) = app.emit_to(
        WINDOW_LABEL,
        ANIMATION_EVENT,
        AnimationsPayload { animations },
    ) {
        log::warn!("fly: cannot re-emit {ANIMATION_EVENT} after a raced idle hide: {error}");
    }
}

/// Delays [`hide_if_idle`] by [`HANDOFF_FADE_MS`] so the overlay can cross-fade over the panel card.
fn schedule_idle_hide(app: AppHandle) {
    let builder = std::thread::Builder::new().name("hangline-fly-handoff".to_string());
    if let Err(error) = builder.spawn(move || {
        std::thread::sleep(Duration::from_millis(HANDOFF_FADE_MS));
        hide_if_idle(&app);
    }) {
        log::warn!("fly: cannot spawn the handoff-hide thread: {error}");
    }
}

/// Whether a watchdog scheduled for `scheduled` may remove the currently active animation.
///
/// An id can be reused for another kind of animation (a card that just landed and is then taken
/// down gets a `fall`), so a stale watchdog must only touch an entry of its own kind.
fn watchdog_matches(active_kind: Option<FlyKind>, scheduled_kind: FlyKind) -> bool {
    active_kind == Some(scheduled_kind)
}

/// Removes an animation that never called `fly_done` (renderer crash, dev HMR reload).
///
/// The watchdog only removes the entry it was scheduled for: an id can be re-used for a later
/// animation with another kind (a card that just landed and is then taken down gets a `fall`), so
/// an unconditional remove would cancel that newer animation and hide the overlay mid-fall.
fn spawn_watchdog(app: AppHandle, id: String, kind: FlyKind, duration_ms: u32) {
    let builder = std::thread::Builder::new().name("hangline-fly-watchdog".to_string());
    let spawned = builder.spawn(move || {
        std::thread::sleep(Duration::from_millis(
            u64::from(duration_ms) + WATCHDOG_MARGIN_MS,
        ));
        let Some(state) = app.try_state::<FlyManager>() else {
            return;
        };
        let removed = {
            let mut guard = state.lock();
            let active_kind = guard.active.get(&id).map(|animation| animation.kind);
            if watchdog_matches(active_kind, kind) {
                guard.active.remove(&id)
            } else {
                // Already finished, or replaced by a newer animation of another kind: leave it.
                None
            }
        };
        let Some(animation) = removed else {
            return;
        };
        log::warn!("fly: watchdog removed a stuck {} animation id={id} (no fly_done)", kind.as_str());
        if animation.kind == FlyKind::Fly {
            if let Err(error) = app.emit_to(
                "panel",
                LANDED_EVENT,
                LandedPayload { id: id.clone() },
            ) {
                log::warn!("fly: watchdog cannot emit {LANDED_EVENT}: {error}");
            }
        }
        hide_if_idle(&app);
    });
    if let Err(error) = spawned {
        log::warn!("fly: cannot spawn the watchdog thread: {error}");
    }
}

/// Starts one `kind` animation; returns `false` when the overlay is not ready or already busy.
fn start_animation(
    app: &AppHandle,
    id: String,
    kind: FlyKind,
    image_path: PathBuf,
    image_version: i64,
    from: Option<PhysicalRect>,
    to: PhysicalRect,
    tilt_deg: f64,
) -> anyhow::Result<bool> {
    // Refuse before creating payloads: a full overlay cannot accept anything anyway.
    {
        let state = app
            .try_state::<FlyManager>()
            .ok_or_else(|| anyhow::anyhow!("fly: the overlay state is not managed"))?;
        let guard = state.lock();
        if !can_start(guard.ready, guard.active.len()) {
            log::info!(
                "fly: refused kind={} (ready={}, active={})",
                kind.as_str(),
                guard.ready,
                guard.active.len()
            );
            return Ok(false);
        }
    }

    let window = ensure_window(app)?;
    // Display change: re-read the virtual screen and resize the overlay before positioning the card.
    // The geometry payload is sent before every flight (not only on a change) so the page also
    // picks up a theme change between flights (`report_fly_ready` only ran once at startup).
    let geometry_changed = sync_geometry(app, &window)?;
    if geometry_changed {
        log::info!("fly: virtual screen changed; notifying the overlay page");
    }
    let payload = GeometryPayload {
        virtual_rect: capture::virtual_screen_rect(),
        theme: theme_name(app),
    };
    if let Err(error) = app.emit_to(WINDOW_LABEL, GEOMETRY_EVENT, payload) {
        log::warn!("fly: cannot emit {GEOMETRY_EVENT}: {error}");
    }

    // Tauri's monitor list is PMV2-correct regardless of the IPC thread; positions/scales must not
    // be virtualized to 100% or the fallback rect and chrome dp would shrink on a scaled display.
    let monitor_list = runtime_monitors(app);
    let target_scale = target_scale_for(to, &monitor_list);
    let from = from.unwrap_or_else(|| fallback_from_rect(to, &monitor_list));
    let animation = FlyAnimation {
        id: id.clone(),
        kind,
        image_path,
        image_version,
        from,
        to,
        tilt_deg,
        duration_ms: kind.duration_ms(),
        target_scale,
    };

    {
        let state = app
            .try_state::<FlyManager>()
            .ok_or_else(|| anyhow::anyhow!("fly: the overlay state is not managed"))?;
        let mut guard = state.lock();
        // Re-check under the lock: a concurrent command may have filled the last slot.
        if !can_start(guard.ready, guard.active.len()) {
            return Ok(false);
        }
        guard.active.insert(id.clone(), animation.clone());
    }
    if let Err(error) = present(app, &window) {
        // Fail open: never leave a non-click-through full-screen overlay on screen.
        log::warn!("fly: rejected kind={} for id={id}: {error}", kind.as_str());
        let state = app.state::<FlyManager>();
        state.lock().active.remove(&id);
        hide_if_idle(app);
        return Ok(false);
    }
    spawn_watchdog(app.clone(), id.clone(), kind, kind.duration_ms());
    log::info!(
        "fly: started id={id} kind={} from=({}, {}) {}x{} to=({}, {}) {}x{} tilt={tilt_deg:.2} targetScale={target_scale:.3}",
        kind.as_str(),
        animation.from.x,
        animation.from.y,
        animation.from.w,
        animation.from.h,
        animation.to.x,
        animation.to.y,
        animation.to.w,
        animation.to.h
    );
    Ok(true)
}

/// Command: fly one new item in from `from` (or the spec fallback) to `to`.
///
/// `image_version` is the panel's `Item.updatedAt`: the overlay renders
/// `assetUrl(imagePath, imageVersion)` so a card edited by the markup editor never flies in with
/// pre-edit bytes from the WebView cache.
#[tauri::command]
pub fn start_fly(
    app: AppHandle,
    id: String,
    from: Option<PhysicalRect>,
    to: PhysicalRect,
    tilt_deg: f64,
    image_version: i64,
) -> Result<bool, String> {
    let image_path = {
        let state = app.state::<AppState>();
        match state.item(&id) {
            Some(item) => item.path.clone(),
            None => {
                log::warn!("fly: item {id} is not in the app state; skipping the flight");
                return Ok(false);
            }
        }
    };
    start_animation(
        &app,
        id,
        FlyKind::Fly,
        image_path,
        image_version.max(0),
        from,
        to,
        tilt_deg,
    )
    .map_err(|error| error.to_string())
}

/// Command: drop one taken-down item from `rect` (P2-6 drives this; renderer lives here).
///
/// `image_version` mirrors `start_fly`: the panel passes the `updatedAt` it last rendered (the item
/// may already be off the line). A taken-down edited card therefore falls with the edited bytes.
#[tauri::command]
pub fn start_fall(
    app: AppHandle,
    id: String,
    image_path: String,
    rect: PhysicalRect,
    tilt_deg: f64,
    image_version: i64,
) -> Result<bool, String> {
    // The panel passes the path from its snapshot because a take-down may already have removed the
    // item from the app state; the overlay renders the last known image, not the state lookup.
    start_animation(
        &app,
        id,
        FlyKind::Fall,
        PathBuf::from(image_path),
        image_version.max(0),
        Some(rect),
        rect,
        tilt_deg,
    )
    .map_err(|error| error.to_string())
}

/// Command: the overlay finished animation `id`; the panel may show a landed card.
#[tauri::command]
pub fn fly_done(app: AppHandle, id: String) -> Result<(), String> {
    let state = app.state::<FlyManager>();
    let finished = {
        let mut guard = state.lock();
        guard.active.remove(&id)
    };
    let Some(animation) = finished else {
        log::debug!("fly: fly_done for unknown id {id} (already finished?)");
        return Ok(());
    };
    match animation.kind {
        FlyKind::Fly => {
            if let Err(error) = app.emit_to("panel", LANDED_EVENT, LandedPayload { id: id.clone() }) {
                log::warn!("fly: cannot emit {LANDED_EVENT}: {error}");
            }
            log::info!("fly: landed id={id}");
            // Keep the overlay for the 0.16 s cross-fade over the now-visible panel card.
            schedule_idle_hide(app.clone());
        }
        FlyKind::Fall => {
            log::info!("fly: fall finished id={id}");
            hide_if_idle(&app);
        }
    }
    Ok(())
}

/// Command: the overlay page registered its listener; returns the virtual-screen rect + theme.
///
/// The page uses it to convert physical pixel coordinates to CSS pixels and to pick the glass
/// theme. Active animations are re-emitted in case the page reloaded (dev HMR, renderer crash).
#[tauri::command]
pub fn report_fly_ready(app: AppHandle) -> Result<GeometryPayload, String> {
    let state = app
        .try_state::<FlyManager>()
        .ok_or_else(|| "fly: the overlay state is not managed".to_string())?;
    let window = state.lock().window.clone();
    // Display change while the page was loading: sync the window **before** marking ready; otherwise
    // the stored virtual_rect would already match the new rect and the resize would never happen.
    // A failure here must not disable flights for the whole session (ready must still be set).
    if let Some(window) = window.as_ref() {
        if let Err(error) = sync_geometry(&app, window) {
            log::warn!("fly: cannot sync the overlay geometry during ready: {error:#}");
        }
    }
    {
        let mut guard = state.lock();
        guard.ready = true;
    }
    let payload = GeometryPayload {
        virtual_rect: capture::virtual_screen_rect(),
        theme: theme_name(&app),
    };
    let animations = {
        let guard = state.lock();
        animation_list(&guard.active)
    };
    if !animations.is_empty() {
        let _ = app.emit_to(WINDOW_LABEL, ANIMATION_EVENT, AnimationsPayload { animations });
    }
    log::info!(
        "fly: overlay ready (virtual screen ({}, {}) {}x{}, theme={})",
        payload.virtual_rect.x,
        payload.virtual_rect.y,
        payload.virtual_rect.w,
        payload.virtual_rect.h,
        payload.theme
    );
    Ok(payload)
}

/// Command: remove and return the capture rect that produced item `id` (spec §4.6 C).
///
/// A miss is a normal case (system screenshots, clipboard, watched folders) and returns `None`;
/// the panel then asks for the fallback origin.
#[tauri::command]
pub fn take_capture_origin(app: AppHandle, id: String) -> Option<PhysicalRect> {
    app.try_state::<AppState>()
        .and_then(|state| state.take_capture_origin(&id))
}

/// Hides the overlay on app exit so a transparent fullscreen window never outlives the app.
pub fn hide_on_exit(app: &AppHandle) {
    let Some(state) = app.try_state::<FlyManager>() else {
        return;
    };
    let window = state.lock().window.clone();
    if let Some(window) = window {
        let _ = window.hide();
    }
}

/// Capture preflight (P4-6): hides the overlay before the interactive capture freezes the screen.
///
/// The pre-warmed overlay is a screen-wide, always-on-top transparent window. While a card is
/// flying — or during the 160 ms handoff cross-fade after it landed — that window is visible, so a
/// `CAPTUREBLT` grab of the virtual screen bakes the overlay copy into the frozen frame: the user
/// then selects a region on a frozen desktop that shows a card which is not really there (the
/// "one-frame flash" from the P2 backlog).
///
/// The active animation list is deliberately kept: an in-flight animation finishes invisibly, its
/// `fly-landed` still reaches the panel (so no panel card stays stuck in `data-flying`), and the
/// next [`present`] re-shows the overlay. A missing overlay window (startup failure, exit) is the
/// normal case and returns `false`; a failed hide is logged and left to the caller's bounded
/// visibility wait, because the capture aborts rather than freezing a tainted screen.
pub fn hide_for_capture(app: &AppHandle) -> bool {
    let Some(state) = app.try_state::<FlyManager>() else {
        return false;
    };
    let window = state.lock().window.clone();
    let Some(window) = window else {
        return false;
    };
    if let Err(error) = window.hide() {
        log::warn!("fly: cannot hide the overlay before the capture freeze: {error}");
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, w: i32, h: i32) -> PhysicalRect {
        PhysicalRect { x, y, w, h }
    }

    fn monitor(id: &str, x: i32, y: i32, w: i32, h: i32, scale: f64, primary: bool) -> MonitorInfo {
        MonitorInfo {
            id: id.to_string(),
            rect: rect(x, y, w, h),
            work_area: rect(x, y, w, h),
            scale_factor: scale,
            is_primary: primary,
        }
    }

    #[test]
    fn can_start_requires_ready_and_an_empty_slot() {
        assert!(can_start(true, 0));
        assert!(can_start(true, MAX_ACTIVE - 1));
        assert!(!can_start(false, 0));
        assert!(!can_start(true, MAX_ACTIVE));
        assert!(!can_start(true, MAX_ACTIVE + 1));
    }

    #[test]
    fn fallback_rect_is_centred_on_the_monitor_containing_the_target_at_its_dpi() {
        let monitors = vec![
            monitor("primary", 0, 0, 1920, 1080, 1.0, true),
            monitor("right", 1920, 0, 2560, 1440, 1.5, false),
        ];
        // Target card on the secondary (150%) monitor: rect centred in that monitor, scaled 1.5.
        let from = fallback_from_rect(rect(2600, 400, 144, 112), &monitors);
        assert_eq!(from.w, dp_to_px(120.0, 1.5));
        assert_eq!(from.h, dp_to_px(80.0, 1.5));
        assert_eq!(from.x, 1920 + (2560 - from.w) / 2);
        assert_eq!(from.y, (1440 - from.h) / 2);

        // Target on the primary (100%): same fallback size at 1dppx, still monitor-centred.
        let primary = fallback_from_rect(rect(800, 200, 144, 112), &monitors);
        assert_eq!(primary.w, 120);
        assert_eq!(primary.h, 80);
        assert_eq!(primary.x, (1920 - 120) / 2);
        assert_eq!(primary.y, (1080 - 80) / 2);
    }

    #[test]
    fn fallback_rect_uses_the_primary_when_the_target_is_off_every_monitor() {
        let monitors = vec![
            monitor("left", -1920, 0, 1920, 1080, 1.25, false),
            monitor("primary", 0, 0, 1920, 1080, 1.0, true),
        ];
        let from = fallback_from_rect(rect(9000, 9000, 10, 10), &monitors);
        assert_eq!(from.w, 120);
        assert_eq!(from.x, (1920 - 120) / 2);
    }

    #[test]
    fn fallback_rect_without_monitors_is_centred_on_the_target() {
        let from = fallback_from_rect(rect(500, 300, 40, 40), &[]);
        assert_eq!(from, rect(500 + 20 - 60, 300 + 20 - 40, 120, 80));
    }

    #[test]
    fn target_scale_is_the_monitor_containing_the_card() {
        let monitors = vec![
            monitor("primary", 0, 0, 1920, 1080, 1.0, true),
            monitor("right", 1920, 0, 2560, 1440, 1.5, false),
        ];
        assert!((target_scale_for(rect(2600, 400, 144, 112), &monitors) - 1.5).abs() < 1e-9);
        assert!((target_scale_for(rect(800, 200, 144, 112), &monitors) - 1.0).abs() < 1e-9);
        // Off-screen targets and empty lists fall back to the primary/default instead of NaN.
        assert!((target_scale_for(rect(0, 0, 10, 10), &monitors) - 1.0).abs() < 1e-9);
        assert_eq!(target_scale_for(rect(0, 0, 10, 10), &[]), 1.0);
    }

    #[test]
    fn animation_list_keeps_btreemap_order_and_camel_case_json() {
        let mut active = BTreeMap::new();
        for (id, kind) in [("b", FlyKind::Fly), ("a", FlyKind::Fall)] {
            active.insert(
                id.to_string(),
                FlyAnimation {
                    id: id.to_string(),
                    kind,
                    image_path: PathBuf::from("C:/x.png"),
                    image_version: 1_700_000_000_900,
                    from: rect(1, 2, 3, 4),
                    to: rect(5, 6, 7, 8),
                    tilt_deg: 1.5,
                    duration_ms: kind.duration_ms(),
                    target_scale: 1.5,
                },
            );
        }
        let list = animation_list(&active);
        assert_eq!(list.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        let json = serde_json::to_value(&list[1]).unwrap();
        assert_eq!(json["id"], "b");
        assert_eq!(json["kind"], "fly");
        assert_eq!(json["imagePath"], "C:/x.png");
        assert_eq!(json["imageVersion"], 1_700_000_000_900i64);
        assert_eq!(json["from"]["x"], 1);
        assert_eq!(json["to"]["h"], 8);
        assert_eq!(json["tiltDeg"], 1.5);
        assert_eq!(json["durationMs"], FLY_DURATION_MS);
        assert_eq!(json["targetScale"], 1.5);
        assert_eq!(list[0].duration_ms, FALL_DURATION_MS);
        assert_eq!(serde_json::to_value(FlyKind::Fall).unwrap(), "fall");
    }

    #[test]
    fn durations_match_the_spec_curves() {
        assert_eq!(FlyKind::Fly.duration_ms(), 650);
        assert_eq!(FlyKind::Fall.duration_ms(), 550);
        assert_eq!(FlyKind::Fly.as_str(), "fly");
        assert_eq!(FlyKind::Fall.as_str(), "fall");
    }

    #[test]
    fn watchdog_only_matches_its_own_kind() {
        // A fly watchdog must not remove a later fall for the same id (and vice versa).
        assert!(watchdog_matches(Some(FlyKind::Fly), FlyKind::Fly));
        assert!(watchdog_matches(Some(FlyKind::Fall), FlyKind::Fall));
        assert!(!watchdog_matches(Some(FlyKind::Fall), FlyKind::Fly));
        assert!(!watchdog_matches(Some(FlyKind::Fly), FlyKind::Fall));
        // Already gone: nothing to remove.
        assert!(!watchdog_matches(None, FlyKind::Fly));
    }
}


