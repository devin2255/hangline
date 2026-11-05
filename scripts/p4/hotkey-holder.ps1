# P4-4 hotkey-conflict runtime helper (dev evidence only).
#
# Registers one global hotkey on this process's (single) thread with `RegisterHotKey` and holds it
# until a stop file appears (or -Seconds elapses), then unregisters it. This is the "another
# application already owns Ctrl+Alt+T" side of M15: the harness starts it, then asks Hangline to
# save the same combination and checks the conflict rollback.
#
#   powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/p4/hotkey-holder.ps1 `
#     -Accelerator "Ctrl+Alt+T" -ReadyPath "$env:TEMP\hk-ready.json" `
#     -StopPath "$env:TEMP\hk-stop" -StatusPath "$env:TEMP\hk-status.json" -Seconds 30
#
# The ready file is the registration result (written immediately, also when registration fails):
#   { "pid": 1234, "accelerator": "Ctrl+Alt+T", "modifiers": 3, "vk": 84,
#     "ok": true, "lastError": 0, "startedAt": 1700000000000 }
# `ok=false` with `lastError=1409` (ERROR_HOTKEY_ALREADY_REGISTERED) is the "probe" mode: it proves
# that another process (Hangline) currently owns the combination.
#
# The status file is written when the helper releases:
#   { "pid": 1234, "ok": true, "unregisterOk": true, "releasedAt": 1700000000123,
#     "stopRequested": true, "heldMs": 1234 }
param(
    [Parameter(Mandatory = $true)][string]$Accelerator,
    [Parameter(Mandatory = $true)][string]$ReadyPath,
    [Parameter(Mandatory = $true)][string]$StopPath,
    [string]$StatusPath = '',
    [int]$Seconds = 30
)

$ErrorActionPreference = 'Stop'

Add-Type -Namespace Hangline -Name HotkeyNative -MemberDefinition @'
[DllImport("user32.dll", SetLastError = true)]
public static extern bool RegisterHotKey(System.IntPtr hWnd, int id, uint fsModifiers, uint vk);

[DllImport("user32.dll", SetLastError = true)]
public static extern bool UnregisterHotKey(System.IntPtr hWnd, int id);
'@

# MOD_ALT = 0x01, MOD_CONTROL = 0x02, MOD_SHIFT = 0x04, MOD_WIN = 0x08.
$modifiers = 0
$vk = 0
foreach ($token in ($Accelerator -split '\+')) {
    $name = $token.Trim()
    if ($name -eq '') { throw "empty token in accelerator '$Accelerator'" }
    $lower = $name.ToLowerInvariant()
    if ($lower -eq 'ctrl' -or $lower -eq 'control') {
        $modifiers = $modifiers -bor 0x0002
    }
    elseif ($lower -eq 'alt' -or $lower -eq 'option') {
        $modifiers = $modifiers -bor 0x0001
    }
    elseif ($lower -eq 'shift') {
        $modifiers = $modifiers -bor 0x0004
    }
    elseif ($lower -eq 'win' -or $lower -eq 'super' -or $lower -eq 'windows' -or $lower -eq 'meta') {
        $modifiers = $modifiers -bor 0x0008
    }
    else {
        if ($vk -ne 0) { throw "more than one main key in accelerator '$Accelerator'" }
        if ($name -match '^[A-Za-z]$') {
            $vk = [int][char]$name.ToUpperInvariant()
        }
        elseif ($name -match '^[0-9]$') {
            $vk = [int][char]$name
        }
        elseif ($name -match '^[Ff]([1-9]|1[0-9]|2[0-4])$') {
            $vk = 0x70 + ([int]$Matches[1] - 1)
        }
        elseif ($lower -eq 'space') { $vk = 0x20 }
        elseif ($lower -eq 'tab') { $vk = 0x09 }
        elseif ($lower -eq 'escape' -or $lower -eq 'esc') { $vk = 0x1B }
        else { throw "unsupported main key '$name' in accelerator '$Accelerator'" }
    }
}
if ($vk -eq 0) { throw "accelerator '$Accelerator' has no main key" }

$startedAt = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
$hotkeyId = 1
$ok = [Hangline.HotkeyNative]::RegisterHotKey([IntPtr]::Zero, $hotkeyId, [uint32]$modifiers, [uint32]$vk)
# `GetLastWin32Error` is only meaningful after a failure; keep the success payload free of the
# stale value the runtime may still carry from an earlier call.
$lastError = if ($ok) { 0 } else { [System.Runtime.InteropServices.Marshal]::GetLastWin32Error() }

$ready = [ordered]@{
    pid         = $PID
    accelerator = $Accelerator
    modifiers   = $modifiers
    vk          = $vk
    ok          = $ok
    lastError   = $lastError
    startedAt   = $startedAt
} | ConvertTo-Json -Compress
[System.IO.File]::WriteAllText($ReadyPath, $ready)

# A failed registration is a complete probe: report it and release the process immediately.
if (-not $ok) {
    if ($StatusPath -ne '') {
        $status = [ordered]@{
            pid           = $PID
            ok            = $false
            unregisterOk  = $false
            releasedAt    = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
            stopRequested = $false
            heldMs        = 0
        } | ConvertTo-Json -Compress
        [System.IO.File]::WriteAllText($StatusPath, $status)
    }
    exit 0
}

# Hold the registration. `RegisterHotKey` is owned by this thread, so the process must stay alive;
# messages are simply left in the queue (a handful for a short evidence run).
$stopRequested = $false
while ((([DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds() - $startedAt) -lt ($Seconds * 1000))) {
    if (Test-Path -LiteralPath $StopPath) {
        $stopRequested = $true
        break
    }
    Start-Sleep -Milliseconds 50
}

$unregisterOk = [Hangline.HotkeyNative]::UnregisterHotKey([IntPtr]::Zero, $hotkeyId)
$releasedAt = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
if ($StatusPath -ne '') {
    $status = [ordered]@{
        pid           = $PID
        ok            = $true
        unregisterOk  = $unregisterOk
        releasedAt    = $releasedAt
        stopRequested = $stopRequested
        heldMs        = $releasedAt - $startedAt
    } | ConvertTo-Json -Compress
    [System.IO.File]::WriteAllText($StatusPath, $status)
}
