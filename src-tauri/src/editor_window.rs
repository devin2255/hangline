//! Shared placement helpers for Hangline's modal editor windows (P3 markup, P3-6 trim).
//!
//! Both editors are borderless, focusable windows created from an `async` Tauri command (off the
//! event-loop thread), sized in CSS pixels and centred on the monitor that holds the panel. The
//! window-specific sizing maths (image aspect fit, video aspect fit) stays in its own module; only
//! the monitor lookup and the physical-placement conversion are shared so the two windows cannot
//! drift apart on multi-DPI desktops.
//!
//! All coordinates are physical pixels of the virtual desktop (design spec §3.9). The monitor's
//! `work_area` and `scale_factor` are authoritative: the panel's own monitor wins, with the
//! pointer/primary monitor as the fallback used by [`crate::panel::target_monitor`].

use anyhow::Context;
use tauri::{AppHandle, Manager, PhysicalPosition, PhysicalSize, WebviewWindow};

use crate::geometry::{self, MonitorInfo};
use crate::panel;

/// Monitor candidate for an editor: the monitor holding the panel, else the pointer/primary.
pub(crate) fn monitor(app: &AppHandle) -> Option<MonitorInfo> {
    let monitors = geometry::monitors();
    let panel_monitor = app
        .get_webview_window(panel::LABEL)
        .and_then(|window| panel::window_rect(&window))
        .and_then(|rect| panel::monitor_for_window(&monitors, rect));
    panel_monitor.or_else(|| panel::target_monitor(&monitors))
}

/// Monitor scale factor, falling back to 100 % for corrupt/zero values.
pub(crate) fn valid_scale(monitor: &MonitorInfo) -> f64 {
    if monitor.scale_factor.is_finite() && monitor.scale_factor > 0.0 {
        monitor.scale_factor
    } else {
        1.0
    }
}

/// Usable work area in CSS pixels (the unit both editor pages lay out in).
///
/// A missing monitor falls back to 1920×1080 (the editor sizing functions clamp to their own
/// minimums, so the page still opens on a corrupt desktop rather than failing the command).
pub(crate) fn work_area_css(monitor: Option<&MonitorInfo>) -> (f64, f64) {
    monitor
        .map(|monitor| {
            let scale = valid_scale(monitor);
            (
                f64::from(monitor.work_area.w.max(1)) / scale,
                f64::from(monitor.work_area.h.max(1)) / scale,
            )
        })
        .unwrap_or((f64::from(1920), f64::from(1080)))
}

/// Applies the size (CSS px → physical px) and centres the window on the monitor's work area.
///
/// `what` names the window in the error messages (`"annotate"`/`"trim"`). The position is applied
/// before the size so the DPI context is the target monitor's (mirrors the panel's own path).
pub(crate) fn apply_geometry(
    window: &WebviewWindow,
    monitor: Option<&MonitorInfo>,
    css_width: f64,
    css_height: f64,
    what: &str,
) -> anyhow::Result<()> {
    let Some(monitor) = monitor else {
        return Ok(());
    };
    let scale = valid_scale(monitor);
    let work_width = u32::try_from(monitor.work_area.w.max(1)).unwrap_or(1);
    let work_height = u32::try_from(monitor.work_area.h.max(1)).unwrap_or(1);
    let width = ((css_width * scale).round().max(1.0) as u32).min(work_width);
    let height = ((css_height * scale).round().max(1.0) as u32).min(work_height);
    let x = monitor.work_area.x + (monitor.work_area.w - width as i32) / 2;
    let y = monitor.work_area.y + (monitor.work_area.h - height as i32) / 2;
    window
        .set_position(PhysicalPosition::new(x, y))
        .with_context(|| format!("{what}: cannot position the editor window"))?;
    window
        .set_size(PhysicalSize::new(width, height))
        .with_context(|| format!("{what}: cannot size the editor window"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor(scale: f64, work: (i32, i32, i32, i32)) -> MonitorInfo {
        MonitorInfo {
            id: "test".to_string(),
            rect: geometry::PhysicalRect {
                x: work.0,
                y: work.1,
                w: work.2,
                h: work.3,
            },
            work_area: geometry::PhysicalRect {
                x: work.0,
                y: work.1,
                w: work.2,
                h: work.3,
            },
            scale_factor: scale,
            is_primary: true,
        }
    }

    #[test]
    fn work_area_is_converted_to_css_pixels() {
        assert_eq!(
            work_area_css(Some(&monitor(1.5, (0, 0, 2560, 1440)))),
            (2560.0 / 1.5, 1440.0 / 1.5)
        );
        assert_eq!(work_area_css(None), (1920.0, 1080.0));
    }

    #[test]
    fn invalid_scale_falls_back_to_100_percent() {
        assert_eq!(valid_scale(&monitor(0.0, (0, 0, 100, 100))), 1.0);
        assert_eq!(valid_scale(&monitor(f64::NAN, (0, 0, 100, 100))), 1.0);
        assert_eq!(valid_scale(&monitor(-2.0, (0, 0, 100, 100))), 1.0);
        assert_eq!(valid_scale(&monitor(1.25, (0, 0, 100, 100))), 1.25);
    }
}
