# P4-4 autostart evidence helper: inspects and restores the two registry locations the
# `tauri-plugin-autostart` / `auto-launch` stack uses on Windows (dev evidence only).
#
#   ... -Action read        -Name Hangline -OutPath state.json
#   ... -Action restore     -Name Hangline -InPath state.json -OutPath after.json
#   ... -Action delete-run  -Name Hangline -OutPath after.json
#   ... -Action set-run     -Name Hangline -Data "C:\...\hangline.exe" -OutPath after.json
#   ... -Action set-approved -Name Hangline -Data "030000000000000000000000" -OutPath after.json
#
# State shape (one entry per hive; `exists=false` means the value is absent):
#   { "run":             { "hkcu": { "exists": false, "kind": "", "data": null }, "hklm": {...} },
#     "startupApproved": { "hkcu": { "exists": true, "kind": "Binary", "data": [2,0,...] }, ... } }
#
# `run` is `HKCU|HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Run` (the login item);
# `startupApproved` is the Task-Manager enabled marker under
# `...\Explorer\StartupApproved\Run` that `AutoLaunch::is_enabled` also reads.
#
# The harness snapshots before touching anything and restores afterwards; it refuses to start when a
# pre-existing Hangline login item is found (the user's real autostart is never modified).
param(
    [Parameter(Mandatory = $true)][ValidateSet('read', 'restore', 'delete-run', 'set-run', 'set-approved')][string]$Action,
    [string]$Name = 'Hangline',
    [string]$InPath = '',
    [string]$OutPath = '',
    [string]$Data = ''
)

$ErrorActionPreference = 'Stop'

$RUN_RUNKEY = 'SOFTWARE\Microsoft\Windows\CurrentVersion\Run'
$STARTUP_APPROVED_RUNKEY = 'SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run'
$ROOTS = [ordered]@{
    hkcu = 'HKEY_CURRENT_USER'
    hklm = 'HKEY_LOCAL_MACHINE'
}

function Get-LocationState([string]$Path, [string]$ValueName) {
    try {
        if (-not (Test-Path -LiteralPath "Registry::$Path")) {
            return [ordered]@{ exists = $false; kind = ''; data = $null }
        }
        $key = Get-Item -LiteralPath "Registry::$Path"
        if (-not ($key.GetValueNames() -contains $ValueName)) {
            return [ordered]@{ exists = $false; kind = ''; data = $null }
        }
        $kind = $key.GetValueKind($ValueName).ToString()
        $value = $key.GetValue($ValueName, $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
        if ($kind -eq 'Binary') {
            $data = @($value | ForEach-Object { [int]$_ })
        }
        elseif ($kind -eq 'MultiString') {
            $data = @($value)
        }
        else {
            $data = [string]$value
        }
        return [ordered]@{ exists = $true; kind = $kind; data = $data }
    }
    catch {
        return [ordered]@{ exists = $false; kind = ''; data = $null; error = "$($_.Exception.Message)" }
    }
}

function Read-State {
    $state = [ordered]@{ run = [ordered]@{}; startupApproved = [ordered]@{} }
    foreach ($hive in $ROOTS.Keys) {
        $root = $ROOTS[$hive]
        $state.run[$hive] = Get-LocationState "$root\$RUN_RUNKEY" $Name
        $state.startupApproved[$hive] = Get-LocationState "$root\$STARTUP_APPROVED_RUNKEY" $Name
    }
    return $state
}

function Set-LocationValue([string]$Path, [string]$ValueName, $Prior) {
    if (-not (Test-Path -LiteralPath "Registry::$Path")) {
        New-Item -Path "Registry::$Path" -Force | Out-Null
    }
    $kind = [string]$Prior.kind
    switch ($kind) {
        'Binary' {
            $bytes = [byte[]]@($Prior.data | ForEach-Object { [byte]$_ })
            New-ItemProperty -LiteralPath "Registry::$Path" -Name $ValueName -Value $bytes -PropertyType Binary -Force | Out-Null
        }
        'DWord' { New-ItemProperty -LiteralPath "Registry::$Path" -Name $ValueName -Value ([int]$Prior.data) -PropertyType DWord -Force | Out-Null }
        'QWord' { New-ItemProperty -LiteralPath "Registry::$Path" -Name $ValueName -Value ([int64]$Prior.data) -PropertyType QWord -Force | Out-Null }
        'MultiString' { New-ItemProperty -LiteralPath "Registry::$Path" -Name $ValueName -Value ([string[]]@($Prior.data)) -PropertyType MultiString -Force | Out-Null }
        'ExpandString' { New-ItemProperty -LiteralPath "Registry::$Path" -Name $ValueName -Value ([string]$Prior.data) -PropertyType ExpandString -Force | Out-Null }
        default { New-ItemProperty -LiteralPath "Registry::$Path" -Name $ValueName -Value ([string]$Prior.data) -PropertyType String -Force | Out-Null }
    }
}

function Remove-LocationValue([string]$Path, [string]$ValueName) {
    if (Test-Path -LiteralPath "Registry::$Path") {
        Remove-ItemProperty -LiteralPath "Registry::$Path" -Name $ValueName -ErrorAction SilentlyContinue
    }
}

function Write-Json($Value, [string]$Path) {
    $json = $Value | ConvertTo-Json -Depth 8 -Compress
    [System.IO.File]::WriteAllText($Path, $json)
}

switch ($Action) {
    'read' {
        Write-Json (Read-State) $OutPath
    }
    'restore' {
        $prior = Get-Content -LiteralPath $InPath -Raw | ConvertFrom-Json
        foreach ($hive in $ROOTS.Keys) {
            $root = $ROOTS[$hive]
            $runPrior = $prior.run.$hive
            if ($runPrior.exists) {
                Set-LocationValue "$root\$RUN_RUNKEY" $Name $runPrior
            }
            else {
                Remove-LocationValue "$root\$RUN_RUNKEY" $Name
            }
            $approvedPrior = $prior.startupApproved.$hive
            if ($approvedPrior.exists) {
                Set-LocationValue "$root\$STARTUP_APPROVED_RUNKEY" $Name $approvedPrior
            }
            else {
                Remove-LocationValue "$root\$STARTUP_APPROVED_RUNKEY" $Name
            }
        }
        Write-Json (Read-State) $OutPath
    }
    'delete-run' {
        foreach ($hive in $ROOTS.Keys) {
            Remove-LocationValue "$($ROOTS[$hive])\$RUN_RUNKEY" $Name
        }
        Write-Json (Read-State) $OutPath
    }
    'set-run' {
        if ($Data -eq '') { throw 'set-run needs -Data' }
        New-ItemProperty -LiteralPath "Registry::HKEY_CURRENT_USER\$RUN_RUNKEY" -Name $Name -Value $Data -PropertyType String -Force | Out-Null
        Write-Json (Read-State) $OutPath
    }
    'set-approved' {
        # Task Manager's state marker: 0x02 = enabled, 0x03 = disabled (plus an optional timestamp).
        if ($Data -eq '' -or ($Data.Length % 2) -ne 0 -or $Data -notmatch '^[0-9a-fA-F]+$') {
            throw 'set-approved needs even-length hexadecimal -Data, e.g. 030000000000000000000000'
        }
        $bytes = [byte[]]::new($Data.Length / 2)
        for ($i = 0; $i -lt $bytes.Length; $i++) {
            $bytes[$i] = [Convert]::ToByte($Data.Substring($i * 2, 2), 16)
        }
        if (-not (Test-Path -LiteralPath "Registry::HKEY_CURRENT_USER\$STARTUP_APPROVED_RUNKEY")) {
            New-Item -Path "Registry::HKEY_CURRENT_USER\$STARTUP_APPROVED_RUNKEY" -Force | Out-Null
        }
        New-ItemProperty -LiteralPath "Registry::HKEY_CURRENT_USER\$STARTUP_APPROVED_RUNKEY" -Name $Name -Value $bytes -PropertyType Binary -Force | Out-Null
        Write-Json (Read-State) $OutPath
    }
}
