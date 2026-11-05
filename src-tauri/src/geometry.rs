//! Pure geometry helpers shared by the panel hit-test and (later) multi-monitor layout.
//!
//! Everything here works with physical ("device") pixels unless stated otherwise, so the
//! hit-test thread and the WebView2 content agree on the same coordinate space.

use std::mem::size_of;

use serde::{Deserialize, Serialize};
use windows::core::BOOL;
use windows::Win32::Foundation::{LPARAM, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFOEXW,
};
use windows::Win32::UI::HiDpi::{
    AreDpiAwarenessContextsEqual, GetDpiForMonitor, GetThreadDpiAwarenessContext,
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, MDT_EFFECTIVE_DPI,
};
use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, MONITORINFOF_PRIMARY};

/// An axis-aligned rectangle in physical pixels.
///
/// For hit rects, `x`/`y` are relative to the panel webview's client origin.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct PhysicalRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl PhysicalRect {
    /// Returns `true` when `point` is inside the rectangle.
    ///
    /// Range semantics are half-open (`min .. max`), consistent with [`any_hit`]: the
    /// left/top edge is inside, the right/bottom edge is outside. Negative origins (monitors
    /// left of or above the primary) work unchanged. Empty rects contain no point.
    pub fn contains(&self, point: (i32, i32)) -> bool {
        self.w > 0
            && self.h > 0
            && point.0 >= self.x
            && point.0 < self.x.saturating_add(self.w)
            && point.1 >= self.y
            && point.1 < self.y.saturating_add(self.h)
    }
}

/// An axis-aligned rectangle in logical pixels (dp), as WebView2 and CSS see it.
#[allow(dead_code)] // Logical view used by the E5 real-session monitor test; the panel is physical.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize)]
pub struct LogicalRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// One monitor in the virtual desktop, in physical pixels.
///
/// `rect` is the full monitor rectangle; `work_area` excludes taskbars/appbars and is the
/// rectangle the panel should be placed against. `scale_factor` is the monitor's effective
/// DPI divided by 96 (1.0 = 100%, 1.5 = 150%). `id` is the GDI device name (e.g.
/// `\\.\DISPLAY1`) so a monitor keeps its identity across `monitors()` calls.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorInfo {
    pub id: String,
    pub rect: PhysicalRect,
    pub work_area: PhysicalRect,
    pub scale_factor: f64,
    pub is_primary: bool,
}

/// Converts a logical (dp) length to physical pixels for the given monitor scale factor.
///
/// Rounds half-way values away from zero, so `4 dp` becomes 4/5/6 px at 100/125/150% instead
/// of losing the hit-test padding to truncation.
pub fn dp_to_px(dp: f64, scale: f64) -> i32 {
    (dp * scale).round() as i32
}

/// Converts a physical-pixel rectangle to logical pixels (dp) at `scale`.
///
/// The result is not rounded: the GUI translates by the resulting f64. `scale` must be the
/// monitor's scale factor (see [`MonitorInfo::scale_factor`]) and greater than zero.
#[allow(dead_code)] // Used by the E5 real-session monitor test; the hit-test only needs dp_to_px.
pub fn to_logical(rect: PhysicalRect, scale: f64) -> LogicalRect {
    LogicalRect {
        x: f64::from(rect.x) / scale,
        y: f64::from(rect.y) / scale,
        w: f64::from(rect.w) / scale,
        h: f64::from(rect.h) / scale,
    }
}

/// Returns `true` when the calling thread uses the Per-Monitor-V2 DPI awareness context.
///
/// Windows virtualizes monitor geometry and `GetDpiForMonitor` for threads that are not PMv2,
/// so [`monitors`] warns when it is called from such a thread. Tauri/tao sets the process to
/// PMv2; newly created runtime threads inherit that context, but a future change (or a unit test
/// binary) can silently regress it, hence the explicit check.
fn thread_is_pmv2() -> bool {
    // SAFETY: both functions are read-only queries with no user pointers.
    unsafe {
        AreDpiAwarenessContextsEqual(
            GetThreadDpiAwarenessContext(),
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        )
        .as_bool()
    }
}

/// Returns the cursor position in physical virtual-desktop pixels.
///
/// Used by the panel to pick its startup monitor and by the debounced (500 ms) pointer follow,
/// which is polled every 200 ms while the panel is visible (Task 14). Like the other geometry
/// calls this assumes a Per-Monitor-V2 thread; see [`monitors`].
pub fn cursor_position() -> Option<(i32, i32)> {
    let mut point = POINT::default();
    // SAFETY: GetCursorPos only writes the POINT it is given.
    match unsafe { GetCursorPos(&mut point) } {
        Ok(()) => Some((point.x, point.y)),
        Err(err) => {
            log::warn!("geometry: GetCursorPos failed ({err})");
            None
        }
    }
}

/// Enumerates the monitors of the current virtual desktop, sorted left-to-right.
///
/// The calling process must be per-monitor DPI aware (Tauri/tao makes the app PMv2); otherwise
/// Windows virtualizes the rectangles *and* `GetDpiForMonitor` results. In particular the
/// `get_monitors` command runs on a runtime thread, so the DPI context is asserted here instead
/// of being assumed silently (Task 5 handoff). Tests that need real values must call
/// `SetProcessDpiAwarenessContext` first (see `capture::tests`).
pub fn monitors() -> Vec<MonitorInfo> {
    if thread_is_pmv2() {
        log::debug!("geometry: enumerating monitors on a Per-Monitor-V2 thread");
    } else {
        log::warn!(
            "geometry: monitors() is not on a Per-Monitor-V2 thread; monitor rectangles and \
             GetDpiForMonitor may be virtualized"
        );
    }
    let mut collected: Vec<MonitorInfo> = Vec::new();
    // SAFETY: the callback runs synchronously inside EnumDisplayMonitors and only touches the
    // Vec behind `data`, which outlives the call.
    let ok = unsafe {
        EnumDisplayMonitors(
            None,
            None,
            Some(enum_monitor),
            LPARAM((&mut collected as *mut Vec<MonitorInfo>) as isize),
        )
    };
    if !ok.as_bool() {
        log::warn!(
            "geometry: EnumDisplayMonitors failed: {}",
            std::io::Error::last_os_error()
        );
    }
    // Enumeration order is not part of the API contract; sort so callers/tests see a stable
    // left-to-right order even when a secondary monitor has a negative x.
    collected.sort_by_key(|monitor| (monitor.rect.x, monitor.rect.y));
    collected
}

/// Returns the monitor containing `point` (physical desktop coordinates).
#[allow(dead_code)] // Live wrapper used by the E5 evidence test; panel placement uses monitor_at_point.
pub fn monitor_at(point: (i32, i32)) -> Option<MonitorInfo> {
    monitor_at_point(&monitors(), point).cloned()
}

/// Pure monitor-at-point lookup over a synthetic or cached monitor list.
///
/// Points in a gap between monitors, or outside the virtual desktop, return `None`; callers
/// that want a nearest-monitor fallback should use the Windows `MonitorFromPoint` API instead.
pub fn monitor_at_point(monitors: &[MonitorInfo], point: (i32, i32)) -> Option<&MonitorInfo> {
    monitors.iter().find(|monitor| monitor.rect.contains(point))
}

/// Returns the work area (taskbar excluded) of the monitor containing `point`.
///
/// A point over a taskbar still belongs to its monitor, so panel placement gets a usable work
/// area even when the cursor sits on the taskbar.
#[allow(dead_code)] // Used by the E5 real-session monitor test; the panel uses whole MonitorInfo.
pub fn work_area_of(monitors: &[MonitorInfo], point: (i32, i32)) -> Option<PhysicalRect> {
    monitor_at_point(monitors, point).map(|monitor| monitor.work_area)
}

/// `EnumDisplayMonitors` callback: appends one [`MonitorInfo`] to the caller's Vec.
unsafe extern "system" fn enum_monitor(
    monitor: HMONITOR,
    _hdc: HDC,
    _clip: *mut RECT,
    data: LPARAM,
) -> BOOL {
    // SAFETY: `data` is the `&mut Vec<MonitorInfo>` passed to EnumDisplayMonitors; the call is
    // synchronous and the Vec outlives it.
    let collected = unsafe { &mut *(data.0 as *mut Vec<MonitorInfo>) };
    match unsafe { monitor_info(monitor) } {
        Some(info) => collected.push(info),
        None => log::warn!("geometry: skipping a monitor whose info could not be read"),
    }
    // Keep enumerating: one unreadable monitor must not hide the others.
    BOOL::from(true)
}

/// Reads one monitor's rectangle, work area, device name, primary flag and effective DPI.
unsafe fn monitor_info(monitor: HMONITOR) -> Option<MonitorInfo> {
    let mut info = MONITORINFOEXW::default();
    // GetMonitorInfoW reads the leading MONITORINFO fields; cbSize tells it that the full
    // MONITORINFOEXW (with the device name) is available.
    info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
    let ok = unsafe { GetMonitorInfoW(monitor, (&mut info as *mut MONITORINFOEXW).cast()) };
    if !ok.as_bool() {
        return None;
    }

    let rect = physical_rect(info.monitorInfo.rcMonitor);
    let work_area = physical_rect(info.monitorInfo.rcWork);
    let mut id = device_name(&info.szDevice);
    if id.is_empty() {
        // Extremely unlikely, but a monitor must still have a stable-ish identifier.
        id = format!("monitor@{},{}", rect.x, rect.y);
    }

    Some(MonitorInfo {
        id,
        rect,
        work_area,
        scale_factor: unsafe { monitor_scale_factor(monitor) },
        is_primary: info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
    })
}

/// Effective DPI scale factor of `monitor` (1.0 = 96 DPI), falling back to 100% on failure.
unsafe fn monitor_scale_factor(monitor: HMONITOR) -> f64 {
    let (mut dpi_x, mut dpi_y) = (0u32, 0u32);
    match unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) } {
        Ok(()) if dpi_x > 0 => f64::from(dpi_x) / 96.0,
        Ok(()) => 1.0,
        Err(err) => {
            log::warn!("geometry: GetDpiForMonitor failed ({err}); assuming 100%");
            1.0
        }
    }
}

/// Converts a Win32 `RECT` to a physical [`PhysicalRect`].
fn physical_rect(rect: RECT) -> PhysicalRect {
    PhysicalRect {
        x: rect.left,
        y: rect.top,
        w: rect.right - rect.left,
        h: rect.bottom - rect.top,
    }
}

/// NUL-terminated UTF-16 monitor device name as a `String`.
fn device_name(device: &[u16; 32]) -> String {
    let len = device.iter().position(|unit| *unit == 0).unwrap_or(32);
    String::from_utf16_lossy(&device[..len])
}

/// Returns `true` when `cursor` is inside any of `rects`, each grown by `pad` pixels.
///
/// Range semantics are half-open (`min .. max`): the left/top edge is inside, the
/// right/bottom edge is outside. That avoids double counting adjacent rects.
pub fn any_hit(cursor: (i32, i32), rects: &[PhysicalRect], pad: i32) -> bool {
    rects.iter().any(|r| {
        cursor.0 >= r.x - pad
            && cursor.0 < r.x + r.w + pad
            && cursor.1 >= r.y - pad
            && cursor.1 < r.y + r.h + pad
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Approximate float comparison for logical-pixel (dp) assertions.
    fn approx(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() < 0.01,
            "expected {expected}, got {actual}"
        );
    }

    /// Three monitors with negative origins on both axes, mirroring a real mixed-DPI rig.
    ///
    /// ```text
    /// +----------------+----------------+
    /// | top   1920x1080 |                |
    /// | 125%  (-1920,-1080)             |
    /// +----------------+ primary        |
    /// | left  1920x1080| 1920x1080 100% |
    /// | 150% (-1920,0) | (0,0)          |
    /// +----------------+----------------+
    /// ```
    fn sample_monitors() -> Vec<MonitorInfo> {
        vec![
            MonitorInfo {
                id: r"\\.\DISPLAY3".to_string(),
                rect: PhysicalRect {
                    x: -1920,
                    y: -1080,
                    w: 1920,
                    h: 1080,
                },
                // Taskbar at the top of the monitor: the work area starts 48 px lower.
                work_area: PhysicalRect {
                    x: -1920,
                    y: -1032,
                    w: 1920,
                    h: 1032,
                },
                scale_factor: 1.25,
                is_primary: false,
            },
            MonitorInfo {
                id: r"\\.\DISPLAY2".to_string(),
                rect: PhysicalRect {
                    x: -1920,
                    y: 0,
                    w: 1920,
                    h: 1080,
                },
                work_area: PhysicalRect {
                    x: -1920,
                    y: 0,
                    w: 1920,
                    h: 1008,
                },
                scale_factor: 1.5,
                is_primary: false,
            },
            MonitorInfo {
                id: r"\\.\DISPLAY1".to_string(),
                rect: PhysicalRect {
                    x: 0,
                    y: 0,
                    w: 1920,
                    h: 1080,
                },
                work_area: PhysicalRect {
                    x: 0,
                    y: 0,
                    w: 1920,
                    h: 1032,
                },
                scale_factor: 1.0,
                is_primary: true,
            },
        ]
    }

    #[test]
    fn dp_to_px_rounds_with_the_monitor_scale() {
        assert_eq!(dp_to_px(4.0, 1.0), 4);
        assert_eq!(dp_to_px(4.0, 1.25), 5);
        assert_eq!(dp_to_px(4.0, 1.5), 6);
        assert_eq!(dp_to_px(4.0, 1.75), 7);
        assert_eq!(dp_to_px(4.0, 2.0), 8);
        // Half-way values round away from zero, so 4 dp never loses its padding.
        assert_eq!(dp_to_px(2.0, 1.25), 3);
        assert_eq!(dp_to_px(1.0, 1.5), 2);
    }

    #[test]
    fn converts_physical_to_logical_with_scale() {
        let r = PhysicalRect {
            x: 100,
            y: 50,
            w: 300,
            h: 200,
        };
        let logical = to_logical(r, 1.5);
        approx(logical.x, 66.67);
        approx(logical.y, 33.33);
        approx(logical.w, 200.0);
        approx(logical.h, 133.33);

        // 100% monitors are a 1:1 mapping.
        assert_eq!(
            to_logical(r, 1.0),
            LogicalRect {
                x: 100.0,
                y: 50.0,
                w: 300.0,
                h: 200.0,
            }
        );
    }

    #[test]
    fn converts_negative_physical_origins_to_logical() {
        let left = PhysicalRect {
            x: -1920,
            y: 0,
            w: 1920,
            h: 1080,
        };
        let logical = to_logical(left, 1.5);
        approx(logical.x, -1280.0);
        approx(logical.y, 0.0);
        approx(logical.w, 1280.0);
        approx(logical.h, 720.0);

        let top = PhysicalRect {
            x: -1920,
            y: -1080,
            w: 1920,
            h: 1080,
        };
        let logical = to_logical(top, 1.25);
        approx(logical.x, -1536.0);
        approx(logical.y, -864.0);
        approx(logical.w, 1536.0);
        approx(logical.h, 864.0);
    }

    #[test]
    fn physical_rect_contains_is_half_open() {
        let r = PhysicalRect {
            x: -50,
            y: -20,
            w: 10,
            h: 10,
        };
        assert!(r.contains((-50, -20)));
        assert!(r.contains((-41, -11)));
        assert!(!r.contains((-40, -10)), "right/bottom edges are outside");
        assert!(!r.contains((-51, -20)));
        // Empty/degenerate rects can never contain a point.
        assert!(!PhysicalRect {
            x: 0,
            y: 0,
            w: 0,
            h: 10
        }
        .contains((0, 0)));
    }

    #[test]
    fn monitor_at_point_handles_negative_origins_and_gaps() {
        let monitors = sample_monitors();
        let id_at = |point: (i32, i32)| monitor_at_point(&monitors, point).map(|m| m.id.as_str());

        // Left monitor (150%, x negative).
        assert_eq!(id_at((-1920, 0)), Some(r"\\.\DISPLAY2"));
        assert_eq!(id_at((-1, 1079)), Some(r"\\.\DISPLAY2"));
        // Top monitor (125%, y negative): the corner is covered by exactly one monitor.
        assert_eq!(id_at((-1920, -1080)), Some(r"\\.\DISPLAY3"));
        assert_eq!(id_at((-1, -1)), Some(r"\\.\DISPLAY3"));
        // Primary.
        assert_eq!(id_at((0, 0)), Some(r"\\.\DISPLAY1"));
        assert_eq!(id_at((1919, 1079)), Some(r"\\.\DISPLAY1"));
        // Outside every monitor (half-open bounds, and beyond the virtual screen).
        assert_eq!(id_at((-1921, 0)), None);
        assert_eq!(id_at((-1920, -1081)), None);
        assert_eq!(id_at((-1, 1080)), None);
        assert_eq!(id_at((1920, 0)), None);
        assert_eq!(id_at((0, 1080)), None);
    }

    #[test]
    fn work_area_of_uses_the_containing_monitors_work_area() {
        let monitors = sample_monitors();

        // Taskbar insets differ per monitor but always come from the monitor, not the point.
        assert_eq!(
            work_area_of(&monitors, (-1920, 0)),
            Some(PhysicalRect {
                x: -1920,
                y: 0,
                w: 1920,
                h: 1008,
            })
        );
        assert_eq!(
            work_area_of(&monitors, (-1, 1007)),
            Some(PhysicalRect {
                x: -1920,
                y: 0,
                w: 1920,
                h: 1008,
            })
        );
        // A point in the taskbar strip still belongs to that monitor.
        assert_eq!(
            work_area_of(&monitors, (-1, 1008)),
            Some(PhysicalRect {
                x: -1920,
                y: 0,
                w: 1920,
                h: 1008,
            })
        );
        assert_eq!(
            work_area_of(&monitors, (0, 1032)),
            Some(PhysicalRect {
                x: 0,
                y: 0,
                w: 1920,
                h: 1032,
            })
        );
        // Top monitor with a top taskbar: the point above the work area is still on it.
        assert_eq!(
            work_area_of(&monitors, (-1920, -1080)),
            Some(PhysicalRect {
                x: -1920,
                y: -1032,
                w: 1920,
                h: 1032,
            })
        );
        // No monitor under the point.
        assert_eq!(work_area_of(&monitors, (1920, 0)), None);
        assert_eq!(work_area_of(&monitors, (-5000, 0)), None);
    }

    #[test]
    fn monitor_info_serializes_camel_case_for_the_ipc_contract() {
        // `get_monitors` returns this shape to the frontend (design spec 4.4); the field names
        // are part of the wire contract, not an implementation detail.
        let monitor = sample_monitors()[2].clone();
        let value = serde_json::to_value(&monitor).expect("MonitorInfo must serialize");
        assert_eq!(value["id"], r"\\.\DISPLAY1");
        assert_eq!(value["rect"]["x"], 0);
        assert_eq!(value["workArea"]["w"], 1920);
        assert_eq!(value["scaleFactor"], 1.0);
        assert_eq!(value["isPrimary"], true);
        assert!(
            value.get("work_area").is_none(),
            "snake_case fields would break the TypeScript mirror"
        );
    }

    #[test]
    fn hit_inside_and_outside() {
        let r = [PhysicalRect {
            x: 100,
            y: 20,
            w: 150,
            h: 120,
        }];
        assert!(any_hit((150, 80), &r, 0));
        assert!(!any_hit((99, 80), &r, 0));
        assert!(any_hit((98, 80), &r, 4)); // 4 px 外扩
        assert!(!any_hit((95, 80), &r, 4));
        assert!(!any_hit((150, 200), &r, 4));
    }

    #[test]
    fn no_rects_never_hits() {
        assert!(!any_hit((0, 0), &[], 4));
    }

    #[test]
    fn edges_are_half_open_and_negative_origins_work() {
        let rects = [
            PhysicalRect {
                x: 100,
                y: 20,
                w: 150,
                h: 120,
            },
            PhysicalRect {
                x: -50,
                y: -20,
                w: 10,
                h: 10,
            },
        ];
        // Left/top edges are inside, right/bottom edges are outside.
        assert!(any_hit((100, 20), &rects, 0));
        assert!(!any_hit((250, 80), &rects, 0));
        assert!(!any_hit((150, 140), &rects, 0));
        // Negative origins (monitor left of / above the primary).
        assert!(any_hit((-50, -20), &rects, 0));
        assert!(!any_hit((-60, -20), &rects, 0));
    }
}
