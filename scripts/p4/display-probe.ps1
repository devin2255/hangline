# P4-3 multi-monitor/DPI runtime probe (dev evidence only).
#
# One short-lived process per query, so every run starts with a fresh DPI awareness context: the
# process is switched to Per-Monitor-V2 *before* any monitor or window API, otherwise Windows
# virtualizes `GetWindowRect`/`GetMonitorInfo` for a DPI-unaware caller and a 150% monitor would
# report logical instead of physical pixels (the panel itself is PMv2).
#
# Modes (all print exactly one JSON document to stdout; no other output):
#
#   monitors     EnumDisplayMonitors + GetMonitorInfoW + GetDpiForMonitor. Each entry is
#                { id, rect, workArea, scaleFactor, isPrimary }. Coordinates are physical virtual-
#                desktop pixels; `workArea` excludes taskbars/appbars.
#   rect         Every top-level window of -ProcessId: { hwnd, pid, className, title, visible,
#                rect, client, clientOrigin, dpi }. The E2E selects the panel by class "Tauri Window".
#   cursor       -X/-Y moves the real pointer with SetCursorPos (the same user input the reveal
#                follow debounce observes); reports { ok, requested, cursor, error }.
#   workstation  Lock state only (no window): QUNS, LogonUI presence, foreground class and whether
#                the input desktop can be opened. On the secure desktop the E2E must fail fast
#                instead of driving a cursor that no user window can follow.
#
#   powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass `
#     -File scripts/p4/display-probe.ps1 -Mode monitors
#
# This is evidence tooling: it reads geometry and moves the real cursor, and never captures the
# desktop or any other application's pixels.
param(
    [ValidateSet('monitors', 'rect', 'cursor', 'workstation')]
    [string]$Mode = 'monitors',
    [int]$ProcessId = 0,
    [int]$X = 0,
    [int]$Y = 0
)

$ErrorActionPreference = 'Stop'

Add-Type @"
using System;
using System.Collections.Generic;
using System.Globalization;
using System.Runtime.InteropServices;
using System.Text;

public static class HanglineDisplayNative {
    public delegate bool MonitorEnumProc(IntPtr hMonitor, IntPtr hdc, IntPtr clip, IntPtr data);
    public delegate bool EnumWindowsProc(IntPtr hwnd, IntPtr data);

    [StructLayout(LayoutKind.Sequential)]
    public struct RECT { public int left; public int top; public int right; public int bottom; }

    [StructLayout(LayoutKind.Sequential)]
    public struct POINT { public int x; public int y; }

    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    public struct MONITORINFOEX {
        public uint cbSize;
        public RECT rcMonitor;
        public RECT rcWork;
        public uint dwFlags;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)] public string szDevice;
    }

    [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr value);
    [DllImport("user32.dll")] public static extern bool EnumDisplayMonitors(IntPtr hdc, IntPtr clip, MonitorEnumProc callback, IntPtr data);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern bool GetMonitorInfoW(IntPtr monitor, ref MONITORINFOEX info);
    [DllImport("shcore.dll")] public static extern int GetDpiForMonitor(IntPtr monitor, int type, out uint dpiX, out uint dpiY);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc callback, IntPtr data);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint pid);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetClassName(IntPtr hwnd, StringBuilder name, int count);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr hwnd, StringBuilder text, int count);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hwnd);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hwnd, out RECT rect);
    [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr hwnd, out RECT rect);
    [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr hwnd, ref POINT point);
    [DllImport("user32.dll", SetLastError = true)] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern bool GetCursorPos(out POINT point);
    [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr hwnd);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll", SetLastError = true)] public static extern IntPtr OpenInputDesktop(uint flags, bool inherit, uint access);
    [DllImport("user32.dll")] public static extern bool CloseDesktop(IntPtr desktop);
    [DllImport("shell32.dll")] public static extern int SHQueryUserNotificationState(out int state);

    public const uint MONITORINFOF_PRIMARY = 1;
    private const int MDT_EFFECTIVE_DPI = 0;

    private static string Q(string value) {
        if (value == null) { value = ""; }
        return "\"" + value.Replace("\\", "\\\\").Replace("\"", "\\\"") + "\"";
    }

    private static string N(double value) {
        return value.ToString("0.####", CultureInfo.InvariantCulture);
    }

    private static string RectJson(RECT rect) {
        return "{\"x\":" + rect.left + ",\"y\":" + rect.top +
            ",\"w\":" + (rect.right - rect.left) + ",\"h\":" + (rect.bottom - rect.top) + "}";
    }

    /// One JSON array entry per monitor in EnumDisplayMonitors order; the caller sorts/classifies.
    public static string MonitorsJson() {
        List<string> items = new List<string>();
        MonitorEnumProc collect = delegate(IntPtr monitor, IntPtr hdc, IntPtr clip, IntPtr data) {
            MONITORINFOEX info = new MONITORINFOEX();
            info.cbSize = (uint)Marshal.SizeOf(typeof(MONITORINFOEX));
            if (!GetMonitorInfoW(monitor, ref info)) { return true; }
            uint dpiX = 0;
            uint dpiY = 0;
            int hr = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, out dpiX, out dpiY);
            double scale = (hr == 0 && dpiX > 0) ? dpiX / 96.0 : 1.0;
            string id = info.szDevice == null ? "" : info.szDevice;
            items.Add("{" +
                "\"id\":" + Q(id) + "," +
                "\"rect\":" + RectJson(info.rcMonitor) + "," +
                "\"workArea\":" + RectJson(info.rcWork) + "," +
                "\"scaleFactor\":" + N(scale) + "," +
                "\"isPrimary\":" + (((info.dwFlags & MONITORINFOF_PRIMARY) != 0) ? "true" : "false") +
                "}");
            return true;
        };
        if (!EnumDisplayMonitors(IntPtr.Zero, IntPtr.Zero, collect, IntPtr.Zero)) {
            return "{\"error\":\"EnumDisplayMonitors failed\"}";
        }
        return "[" + string.Join(",", items.ToArray()) + "]";
    }

    /// Every top-level window owned by `pid`; the caller classifies them (class + title).
    public static string WindowsJson(int pid) {
        List<string> items = new List<string>();
        EnumWindowsProc collect = delegate(IntPtr hwnd, IntPtr data) {
            uint owner = 0;
            GetWindowThreadProcessId(hwnd, out owner);
            if (owner != (uint)pid) { return true; }
            RECT rect;
            RECT client;
            GetWindowRect(hwnd, out rect);
            GetClientRect(hwnd, out client);
            POINT origin = new POINT();
            ClientToScreen(hwnd, ref origin);
            StringBuilder cls = new StringBuilder(256);
            GetClassName(hwnd, cls, 256);
            StringBuilder title = new StringBuilder(512);
            GetWindowTextW(hwnd, title, 512);
            items.Add("{" +
                "\"hwnd\":" + hwnd.ToInt64() + "," +
                "\"pid\":" + pid + "," +
                "\"className\":" + Q(cls.ToString()) + "," +
                "\"title\":" + Q(title.ToString()) + "," +
                "\"visible\":" + (IsWindowVisible(hwnd) ? "true" : "false") + "," +
                "\"rect\":" + RectJson(rect) + "," +
                "\"client\":{\"w\":" + (client.right - client.left) + ",\"h\":" + (client.bottom - client.top) + "}," +
                "\"clientOrigin\":{\"x\":" + origin.x + ",\"y\":" + origin.y + "}," +
                "\"dpi\":" + GetDpiForWindow(hwnd) +
                "}");
            return true;
        };
        if (!EnumWindows(collect, IntPtr.Zero)) {
            return "{\"error\":\"EnumWindows failed\"}";
        }
        return "[" + string.Join(",", items.ToArray()) + "]";
    }

    /// Moves the real pointer and reports where Windows actually put it.
    public static string CursorJson(int x, int y) {
        bool ok = SetCursorPos(x, y);
        int error = ok ? 0 : Marshal.GetLastWin32Error();
        POINT point;
        bool read = GetCursorPos(out point);
        return "{\"ok\":" + (ok ? "true" : "false") + "," +
            "\"requested\":{\"x\":" + x + ",\"y\":" + y + "}," +
            "\"cursor\":" + (read ? "{\"x\":" + point.x + ",\"y\":" + point.y + "}" : "null") + "," +
            "\"error\":" + error + "}";
    }

    /// Lock-state probe. `inputDesktopAccessible=false` (or LogonUI/QUNS=1) means the secure
    /// desktop owns the session: cursor moves cannot be observed by a normal user window.
    public static string WorkstationJson() {
        int state = 0;
        int hr = SHQueryUserNotificationState(out state);
        IntPtr foreground = GetForegroundWindow();
        StringBuilder cls = new StringBuilder(256);
        if (foreground != IntPtr.Zero) { GetClassName(foreground, cls, 256); }
        IntPtr desktop = OpenInputDesktop(0, false, 0x0001 | 0x0100);
        bool accessible = desktop != IntPtr.Zero;
        int error = accessible ? 0 : Marshal.GetLastWin32Error();
        if (accessible) { CloseDesktop(desktop); }
        return "{" +
            "\"quns\":" + state + "," +
            "\"qunsHr\":" + hr + "," +
            "\"foregroundClass\":" + Q(cls.ToString()) + "," +
            "\"inputDesktopAccessible\":" + (accessible ? "true" : "false") + "," +
            "\"inputDesktopError\":" + error +
            "}";
    }
}
"@

# DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2 = -4. It must be applied before the first geometry
# query: a DPI-unaware caller receives DPI-virtualized rectangles and DPI values.
[void][HanglineDisplayNative]::SetProcessDpiAwarenessContext([IntPtr](-4))

$payload = switch ($Mode) {
    'monitors' { [HanglineDisplayNative]::MonitorsJson() }
    'rect' {
        if ($ProcessId -le 0) { '{"error":"-ProcessId is required for rect mode"}' }
        else { [HanglineDisplayNative]::WindowsJson($ProcessId) }
    }
    'cursor' { [HanglineDisplayNative]::CursorJson($X, $Y) }
    'workstation' { [HanglineDisplayNative]::WorkstationJson() }
}

[Console]::Out.Write($payload)
