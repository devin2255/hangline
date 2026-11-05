# P4-2 full-screen suppression runtime helper (dev evidence only).
#
# Creates one synthetic window and keeps it foreground until a stop file appears (or -Seconds
# elapses). Modes:
#
#   fullscreen  (default) borderless, topmost window exactly covering the primary monitor. This
#               exercises the "foreground window covers its monitor" branch of
#               `src-tauri/src/fullscreen.rs` (a neutral synthetic surface; no desktop capture, no
#               user content).
#   maximized   ordinary framed window maximized on the primary monitor.
#   maximized-secondary   ordinary framed window maximized on a secondary monitor.
#   covering    ordinary framed window manually sized to cover the secondary monitor (the rect-only
#               false positive: it covers the monitor rectangle but has a title bar/frame).
#   borderless-max   borderless window maximized to the secondary monitor's full rectangle (a
#               shell-managed maximized window → must not suppress even though it is borderless).
#   probe       no window: report the workstation lock state (QUNS + LogonUI + foreground class)
#               and exit, so the E2E can fail fast instead of driving a secure desktop.
#
#   powershell.exe -NoProfile -STA -ExecutionPolicy Bypass -File scripts/p4/fullscreen-window.ps1 `
#     -ReadyPath "$env:TEMP\fs-ready.json" -StopPath "$env:TEMP\fs-stop" -Seconds 30
#
# The ready file is written only after the window really is the foreground window, so the E2E can
# measure suppression/restore latency from a trustworthy t0:
#   { "pid": 1234, "hwnd": 123456, "mode": "fullscreen", "className": "WindowsForms10...",
#     "style": 349110272, "exStyle": 8, "showCmd": 1, "borderless": true,
#     "maximizedPlacement": false, "bounds": { "x":0, "y":0, "w":1920, "h":1080 },
#     "monitor": { "x":0, "y":0, "w":1920, "h":1080 },
#     "startedAt": 1700000000000, "foregroundAt": 1700000000100 }
# The `.closed` marker (`<StopPath>.closed`) carries the epoch milliseconds immediately after the
# native window was destroyed; the E2E refuses to measure the restore without it (no wall-clock
# substitute).
param(
    [Parameter(Mandatory = $true)][string]$ReadyPath,
    [Parameter(Mandatory = $true)][string]$StopPath,
    [int]$Seconds = 30,
    # Optional diagnostics: rewritten every second with the form/foreground state so a failed
    # foreground takeover can be diagnosed from the harness.
    [string]$StatusPath = '',
    [ValidateSet('fullscreen', 'maximized', 'maximized-secondary', 'covering', 'borderless-max', 'probe')]
    [string]$Mode = 'fullscreen'
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class HanglineFullscreenNative {
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetClassName(IntPtr hWnd, StringBuilder name, int count);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern void SwitchToThisWindow(IntPtr hWnd, bool altTab);
    [DllImport("user32.dll")] public static extern bool BringWindowToTop(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr value);
    [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr hWnd, IntPtr insertAfter, int x, int y, int cx, int cy, uint flags);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint processId);
    [DllImport("user32.dll")] public static extern int GetWindowLong(IntPtr hWnd, int nIndex);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT rect);
    [DllImport("user32.dll")] public static extern bool GetWindowPlacement(IntPtr hWnd, ref WINDOWPLACEMENT placement);
    [DllImport("kernel32.dll")] public static extern uint GetCurrentThreadId();
    [DllImport("user32.dll")] public static extern void keybd_event(byte virtualKey, byte scan, uint flags, UIntPtr extraInfo);
    [DllImport("shell32.dll")] public static extern int SHQueryUserNotificationState(out int state);

    public const int GWL_STYLE = -16;
    public const int GWL_EXSTYLE = -20;
    public const uint SW_SHOWMAXIMIZED = 3;
    public const uint WS_CAPTION = 0x00C00000;
    public const uint WS_THICKFRAME = 0x00040000;

    [StructLayout(LayoutKind.Sequential)]
    public struct RECT { public int left; public int top; public int right; public int bottom; }

    [StructLayout(LayoutKind.Sequential)]
    public struct POINT { public int x; public int y; }

    [StructLayout(LayoutKind.Sequential)]
    public struct WINDOWPLACEMENT {
        public uint length;
        public uint flags;
        public uint showCmd;
        public POINT ptMinPosition;
        public POINT ptMaxPosition;
        public RECT rcNormalPosition;
    }

    public static string ClassName(IntPtr hwnd) {
        if (hwnd == IntPtr.Zero) { return ""; }
        StringBuilder name = new StringBuilder(256);
        GetClassName(hwnd, name, 256);
        return name.ToString();
    }

    /// Style bits plus the placement of one window, as evidence for the E2E classifier.
    public static string Describe(IntPtr hwnd) {
        RECT rect;
        GetWindowRect(hwnd, out rect);
        WINDOWPLACEMENT placement = new WINDOWPLACEMENT();
        placement.length = (uint)Marshal.SizeOf(typeof(WINDOWPLACEMENT));
        bool placed = GetWindowPlacement(hwnd, ref placement);
        uint style = unchecked((uint)GetWindowLong(hwnd, GWL_STYLE));
        uint exStyle = unchecked((uint)GetWindowLong(hwnd, GWL_EXSTYLE));
        bool borderless = (style & (WS_CAPTION | WS_THICKFRAME)) == 0;
        bool maximized = placed && placement.showCmd == SW_SHOWMAXIMIZED;
        return "{" +
            "\"className\":" + Quote(ClassName(hwnd)) + "," +
            "\"style\":" + style + "," +
            "\"exStyle\":" + exStyle + "," +
            "\"showCmd\":" + (placed ? placement.showCmd : 0) + "," +
            "\"borderless\":" + (borderless ? "true" : "false") + "," +
            "\"maximizedPlacement\":" + (maximized ? "true" : "false") + "," +
            "\"bounds\":{\"x\":" + rect.left + ",\"y\":" + rect.top + ",\"w\":" + (rect.right - rect.left) + ",\"h\":" + (rect.bottom - rect.top) + "}" +
            "}";
    }

    private static string Quote(string value) {
        return "\"" + value.Replace("\\", "\\\\").Replace("\"", "\\\"") + "\"";
    }

    /// A harmless Alt tap: the foreground lock is released for a process that just received input.
    /// Only used by the evidence helper when direct activation is refused.
    public static void NudgeAlt() {
        keybd_event(0x12, 0, 0, UIntPtr.Zero);
        keybd_event(0x12, 0, 2, UIntPtr.Zero);
    }

    /// Classic foreground-forcing sequence: attach to the current foreground thread, bring our
    /// window to the top and activate it, then detach. The timer retries until GetForegroundWindow
    /// reports our handle (the ready file is only written after that).
    public static bool ForceForeground(IntPtr hwnd) {
        IntPtr foreground = GetForegroundWindow();
        uint ignored = 0;
        uint foregroundThread = foreground == IntPtr.Zero ? 0 : GetWindowThreadProcessId(foreground, out ignored);
        uint currentThread = GetCurrentThreadId();
        bool attached = false;
        if (foregroundThread != 0 && foregroundThread != currentThread) {
            attached = AttachThreadInput(currentThread, foregroundThread, true);
        }
        BringWindowToTop(hwnd);
        bool foregrounded = SetForegroundWindow(hwnd);
        if (!foregrounded) {
            // SetForegroundWindow is refused for a process that does not own the foreground and
            // was not started by it (a Node/PowerShell-spawned evidence helper). SwitchToThisWindow
            // is the taskbar's own activation path and does not need the foreground lock.
            SwitchToThisWindow(hwnd, true);
            foregrounded = GetForegroundWindow() == hwnd;
        }
        if (attached) {
            AttachThreadInput(currentThread, foregroundThread, false);
        }
        return foregrounded;
    }

    [DllImport("user32.dll")] private static extern bool AttachThreadInput(uint idAttach, uint idAttachTo, bool attach);
}
"@

# DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2 = -4. Without it the form's Bounds are DPI-virtualized
# in this PowerShell process while Hangline (PMv2) compares physical pixels.
[void][HanglineFullscreenNative]::SetProcessDpiAwarenessContext([IntPtr](-4))
[System.Windows.Forms.Application]::EnableVisualStyles()

# `Form.MaximizedBounds` is protected; this tiny subclass exposes it so the negative-case helpers
# can maximize to the *monitor* rectangle (the geometry a maximized window has on a monitor
# without a taskbar) instead of the work area.
$winFormsAssembly = [System.Windows.Forms.Form].Assembly.Location
$drawingAssembly = [System.Drawing.Rectangle].Assembly.Location
Add-Type -TypeDefinition @"
using System.Drawing;
using System.Windows.Forms;
public class HanglineProbeForm : Form {
    public void SetMaximizedRectangle(Rectangle rectangle) { MaximizedBounds = rectangle; }
}
"@ -ReferencedAssemblies @($winFormsAssembly, $drawingAssembly)

$now = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
$startedAt = $now

# Probe mode: never shows a window. The E2E refuses to run on a locked workstation, where no
# user-window can take the foreground and QUNS is forced to 1 (QUNS_NOT_PRESENT).
if ($Mode -eq 'probe') {
    $state = 0
    $hr = [HanglineFullscreenNative]::SHQueryUserNotificationState([ref]$state)
    $foreground = [HanglineFullscreenNative]::GetForegroundWindow()
    $logon = $false
    try { $logon = [bool](Get-Process LogonUI -ErrorAction SilentlyContinue) } catch { }
    $payload = [ordered]@{
        mode           = 'probe'
        quns           = $state
        qunsHr         = $hr
        foreground     = $foreground.ToInt64()
        foregroundClass = [HanglineFullscreenNative]::ClassName($foreground)
        logonUi        = $logon
        checkedAt      = $now
    } | ConvertTo-Json -Compress
    [System.IO.File]::WriteAllText($ReadyPath, $payload)
    exit 0
}

$allScreens = [System.Windows.Forms.Screen]::AllScreens
$primary = [System.Windows.Forms.Screen]::PrimaryScreen
$secondary = $allScreens | Where-Object { -not $_.Primary } | Select-Object -First 1

$target = $primary
if ($Mode -eq 'maximized-secondary' -or $Mode -eq 'borderless-max' -or $Mode -eq 'covering') {
    if ($null -eq $secondary) {
        $payload = [ordered]@{
            mode  = $Mode
            error = "mode '$Mode' needs a secondary monitor and none is attached"
        } | ConvertTo-Json -Compress
        [System.IO.File]::WriteAllText($ReadyPath, $payload)
        exit 2
    }
    $target = $secondary
}

$bounds = $target.Bounds
$form = New-Object HanglineProbeForm
$form.FormBorderStyle = if ($Mode -eq 'fullscreen' -or $Mode -eq 'borderless-max') {
    [System.Windows.Forms.FormBorderStyle]::None
} else {
    [System.Windows.Forms.FormBorderStyle]::Sizable
}
$form.StartPosition = [System.Windows.Forms.FormStartPosition]::Manual
$form.Bounds = New-Object System.Drawing.Rectangle($bounds.X, $bounds.Y, $bounds.Width, $bounds.Height)
# Only the synthetic full-screen stand-in is topmost: the negative cases must behave like an
# ordinary application window and must not cover the line visually.
$form.TopMost = ($Mode -eq 'fullscreen')
$form.ShowInTaskbar = ($Mode -ne 'fullscreen')
$form.BackColor = [System.Drawing.Color]::FromArgb(12, 14, 18)
$form.Text = 'Hangline full-screen probe'

$script:ready = $false
$script:foregroundAt = 0
$script:nudges = 0

$form.Show()
if ($Mode -eq 'covering') {
    # An ordinary framed window manually sized over the monitor: it covers the monitor rectangle
    # (the rect-only false positive) but has a title bar and resize frame, so it is not full screen.
    [void][HanglineFullscreenNative]::SetWindowPos(
        $form.Handle, [IntPtr]::Zero, $bounds.X, $bounds.Y, $bounds.Width, $bounds.Height,
        (0x0004 -bor 0x0010)) # SWP_NOZORDER | SWP_NOACTIVATE
}
if ($Mode -ne 'fullscreen' -and $Mode -ne 'covering') {
    if ($Mode -eq 'maximized-secondary' -or $Mode -eq 'borderless-max') {
        # Both attached monitors currently have a taskbar, so a plain maximize would stop short of
        # the monitor and could never reproduce the rect-only false positive. Maximizing to the
        # monitor rectangle is exactly what a maximized window looks like on a taskbar-less monitor
        # (and the geometry that used to fool `covers_rect`). The rectangle is monitor-relative.
        $form.SetMaximizedRectangle(
            (New-Object System.Drawing.Rectangle(0, 0, $bounds.Width, $bounds.Height)))
    }
    # Maximizing after the window exists makes WinForms maximize on the window's current monitor
    # (the secondary for the "-secondary" modes), not on the primary.
    $form.WindowState = [System.Windows.Forms.FormWindowState]::Maximized
}
$form.Activate()
[void][HanglineFullscreenNative]::ForceForeground($form.Handle)

$timer = New-Object System.Windows.Forms.Timer
$timer.Interval = 100
$timer.Add_Tick({
        $tickNow = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
        if ($StatusPath -ne '') {
            $foreground = [HanglineFullscreenNative]::GetForegroundWindow()
            $status = [ordered]@{
                pid              = $PID
                mode             = $Mode
                hwnd             = $form.Handle.ToInt64()
                visible          = $form.Visible
                foreground       = $foreground.ToInt64()
                foregroundClass  = [HanglineFullscreenNative]::ClassName($foreground)
                isForeground     = ($foreground -eq $form.Handle)
                now              = $tickNow
            } | ConvertTo-Json -Compress
            try { [System.IO.File]::WriteAllText($StatusPath, $status) } catch { }
        }
        if (-not $script:ready) {
            if ($script:nudges -lt 3) {
                # A process that just received input may take the foreground; harmless Alt tap.
                [HanglineFullscreenNative]::NudgeAlt()
                $script:nudges += 1
            }
            [void][HanglineFullscreenNative]::ForceForeground($form.Handle)
        }
        if (-not $script:ready -and [HanglineFullscreenNative]::GetForegroundWindow() -eq $form.Handle) {
            $script:ready = $true
            $script:foregroundAt = $tickNow
            $described = [HanglineFullscreenNative]::Describe($form.Handle)
            $payload = '{"pid":' + $PID + ',"mode":"' + $Mode + '","hwnd":' + $form.Handle.ToInt64() +
                ',"monitor":{"x":' + $bounds.X + ',"y":' + $bounds.Y + ',"w":' + $bounds.Width + ',"h":' + $bounds.Height + '}' +
                ',"startedAt":' + $startedAt + ',"foregroundAt":' + $tickNow + ',' + $described.Substring(1)
            [System.IO.File]::WriteAllText($ReadyPath, $payload)
        }
        if ($script:ready) {
            # Keep the synthetic full-screen window in front of the always-on-top panel.
            [void][HanglineFullscreenNative]::ForceForeground($form.Handle)
            if ($Mode -eq 'fullscreen') {
                [void][HanglineFullscreenNative]::SetWindowPos($form.Handle, [IntPtr](-1), 0, 0, 0, 0, 0x0003)
            }
        }
        if ((Test-Path -LiteralPath $StopPath) -or (($tickNow - $startedAt) -gt ($Seconds * 1000))) {
            $timer.Stop()
            # Close first, then timestamp: `Form.Close` destroys the native window synchronously, so
            # this marker is as close as possible to the moment the full-screen window disappears.
            # The harness fails (never substitutes a wall-clock time) when this marker is missing.
            $form.Close()
            try {
                [System.IO.File]::WriteAllText(
                    "$StopPath.closed",
                    "$([DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds())")
            } catch { }
        }
    })
$timer.Start()
[System.Windows.Forms.Application]::Run($form)
