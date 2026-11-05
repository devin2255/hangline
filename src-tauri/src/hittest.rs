//! 30 Hz cursor polling that toggles the panel's click-through state.
//!
//! The frontend reports card rects in physical pixels; this loop converts the global
//! cursor position into panel-local pixels and only calls the window API when the
//! desired state actually changes (no per-tick `set_ignore_cursor_events` churn).
//!
//! While the panel is hidden the loop reads the cached `panel::is_visible()` flag and sleeps
//! for [`HIDDEN_INTERVAL`] without touching the window at all (design spec 4.6).
//!
//! While a native OLE drag-out session is in flight (`dragout::is_dragging`) the loop leaves
//! the click-through state untouched: the OS drag loop owns the mouse and must not be
//! interrupted.

use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

use tauri::{AppHandle, Manager};

use crate::dragout;
use crate::geometry::{any_hit, dp_to_px, PhysicalRect};
use crate::panel;

/// Poll cadence from the design spec while the panel is visible: ~30 Hz.
pub const POLL_INTERVAL: Duration = Duration::from_millis(33);

/// Idle cadence while the panel is hidden: no window queries, only a cheap flag check.
pub const HIDDEN_INTERVAL: Duration = Duration::from_millis(200);

/// Card rects are grown by this many logical pixels (dp) before hit-testing.
///
/// The frontend's 4 dp threshold is DPI-independent; the poll loop converts it to physical
/// pixels for the panel's current monitor scale (4/5/6 px at 100/125/150%).
pub const HIT_PADDING_DP: f64 = 4.0;

/// Converts [`HIT_PADDING_DP`] to physical pixels for a panel scale factor.
fn hit_padding(scale: f64) -> i32 {
    dp_to_px(HIT_PADDING_DP, scale)
}

/// Card hit rects shared between the `set_hit_rects` command and the poll loop.
#[derive(Clone, Default)]
pub struct HitRects(Arc<Mutex<Vec<PhysicalRect>>>);

impl HitRects {
    /// Replaces the hit rects; called from IPC whenever the panel layout changes.
    pub fn set(&self, rects: Vec<PhysicalRect>) {
        *self.lock() = rects;
    }

    fn lock(&self) -> MutexGuard<'_, Vec<PhysicalRect>> {
        // A panic elsewhere must not permanently disable click-through switching.
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Starts the hit-test thread. It lives for the whole app lifetime.
pub fn spawn(app: AppHandle, rects: HitRects) {
    thread::spawn(move || run(app, rects));
}

fn run(app: AppHandle, rects: HitRects) {
    // `panel::create` starts with click-through enabled; keep the cache in sync with it.
    let mut ignoring = true;

    loop {
        // Hidden: skip every window query. `hide` also restores click-through, so reset the
        // cache to match it and re-evaluate the real state on the first visible tick.
        if !panel::is_visible() {
            ignoring = true;
            thread::sleep(HIDDEN_INTERVAL);
            continue;
        }

        thread::sleep(POLL_INTERVAL);

        // An OS drag session owns the mouse while it is running. Leave `ignore_cursor_events`
        // exactly as it is: flipping the panel to click-through can cancel the drag feedback
        // (and would steal the drop target under the cursor).
        if dragout::is_dragging() {
            continue;
        }

        let Some(panel) = app.get_webview_window(panel::LABEL) else {
            continue;
        };

        let (Ok(cursor), Ok(origin)) = (panel.cursor_position(), panel.outer_position()) else {
            continue;
        };
        // Both positions are physical pixels. The webview client origin equals the outer
        // origin for this undecorated, shadow-less window.
        let local = (cursor.x as i32 - origin.x, cursor.y as i32 - origin.y);
        // `scale_factor` follows the monitor hosting the panel, so the 4 dp padding stays
        // 4 logical pixels on 150% monitors (6 physical pixels) instead of shrinking.
        let scale = panel.scale_factor().unwrap_or(1.0);
        let hit = {
            let guard = rects.lock();
            any_hit(local, &guard, hit_padding(scale))
        };
        let should_ignore = !hit;

        if should_ignore != ignoring && panel.set_ignore_cursor_events(should_ignore).is_ok() {
            ignoring = should_ignore;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hit_padding_follows_the_panel_scale() {
        assert_eq!(hit_padding(1.0), 4);
        assert_eq!(hit_padding(1.25), 5);
        assert_eq!(hit_padding(1.5), 6);
    }
}
