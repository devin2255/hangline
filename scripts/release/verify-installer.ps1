#!/usr/bin/env pwsh
# P5-5: safe local installer/portable lifecycle evidence.
#
# The production installer writes to the real per-user locations (%LOCALAPPDATA%\Programs\Hangline,
# Start Menu, HKCU Uninstall) and its uninstaller can delete %LOCALAPPDATA%\Hangline. To avoid
# touching a developer's real installation/data, this script compiles a **test-only build of the
# same installer.nsi** with /D overrides that relocate the install dir, data dir and shortcut dirs
# under %TEMP%, then exercises install -> launch -> upgrade -> uninstall keep/delete -> portable.
# The production artifacts are additionally validated statically (size/hash/version/signature/
# manifest) and the script asserts the real install/data locations were untouched.
#
# Usage:
#   pwsh -NoProfile -File scripts/release/verify-installer.ps1
# Optional: -Setup <path> -Portable <path> -Version <x.y.z> -KeepBase
# Exit code 0 = all checks passed; 1 = failure (details in the JSON).

[CmdletBinding()]
param(
  [string]$Setup = "",
  [string]$Portable = "",
  [string]$Version = "",
  [string]$UpgradeFromVersion = "0.4.0",
  [string]$BaselinePortable = "",
  [switch]$KeepBase
)

$ErrorActionPreference = "Stop"
$repo = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
if (-not $Version) { $Version = (Get-Content (Join-Path $repo "package.json") -Raw | ConvertFrom-Json).version }
if (-not $Setup) { $Setup = Join-Path $repo "release\Hangline_${Version}_x64-setup.exe" }
if (-not $Portable) { $Portable = Join-Path $repo "release\Hangline_${Version}_x64-portable.zip" }
if (-not $BaselinePortable) {
  $candidates = @(
    (Join-Path $repo "release\Hangline_${UpgradeFromVersion}_x64-portable.zip"),
    (Join-Path $repo ".superpowers\p5-evidence\old-${UpgradeFromVersion}\Hangline_${UpgradeFromVersion}_x64-portable.zip")
  )
  $BaselinePortable = ($candidates | Where-Object { Test-Path $_ } | Select-Object -First 1)
  if (-not $BaselinePortable) { $BaselinePortable = $candidates[0] }
}
$base = Join-Path $env:TEMP "hangline-p5-lifecycle"
$evidenceDir = Join-Path $repo ".superpowers\p5-evidence\installer"
New-Item -ItemType Directory -Force $evidenceDir | Out-Null

$results = [ordered]@{ startedAt = (Get-Date).ToString("o"); version = $Version; checks = [ordered]@{}; steps = @(); errors = @() }
# Test-only HKCU locations: the lifecycle build never writes the real Uninstall/Run/StartupApproved keys.
$script:testUninstallKeyPath = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\HanglineP5Test"
$script:testUninstallKeyName = "Software\Microsoft\Windows\CurrentVersion\Uninstall\HanglineP5Test"
$script:testRunKeyName = "Software\Microsoft\Windows\CurrentVersion\RunP5Test"
$script:testStartupKeyName = "Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\RunP5Test"
$script:realUninstallKeyPath = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Hangline"
$script:realRunKeyPath = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run"
$script:realStartupKeyPath = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run"
$failures = 0
function Check([string]$name, [bool]$ok, $detail = $null) {
  $script:results.checks[$name] = @{ pass = [bool]$ok; detail = $detail }
  if (-not $ok) { $script:failures++; Write-Host "FAIL $name : $detail" -ForegroundColor Red }
  else { Write-Host "PASS $name" }
}
function Step([string]$name, $detail = $null) {
  $script:results.steps += @{ name = $name; at = (Get-Date).ToString("o"); detail = $detail }
  Write-Host "step: $name"
}
function Kill-Tree([int]$processId) {
  if ($processId -le 0) { return }
  & taskkill /PID $processId /T /F 2>$null | Out-Null
  Start-Sleep -Milliseconds 300
}
function Wait-For([scriptblock]$condition, [int]$timeoutMs = 15000, [string]$label = "condition") {
  $deadline = (Get-Date).AddMilliseconds($timeoutMs)
  do {
    if (& $condition) { return $true }
    Start-Sleep -Milliseconds 200
  } while ((Get-Date) -lt $deadline)
  throw "timed out waiting for $label"
}
function Get-RegUninstall { Get-ItemProperty -Path $script:testUninstallKeyPath -ErrorAction SilentlyContinue }
function Test-RealUntouched([string]$label) {
  # Real user data/install must not be created/removed by this lifecycle run. Use the original
  # (pre-isolation) LOCALAPPDATA captured below, never the per-launch isolated env value.
  $realInstall = Join-Path $script:realLocalAppData "Programs\Hangline"
  $realData = Join-Path $script:realLocalAppData "Hangline"
  $afterInstall = Test-Path $realInstall
  $afterData = Test-Path $realData
  $runValue = (Get-ItemProperty -Path $script:realRunKeyPath -Name Hangline -ErrorAction SilentlyContinue)
  $startupValue = (Get-ItemProperty -Path $script:realStartupKeyPath -Name Hangline -ErrorAction SilentlyContinue)
  $uninstallKey = (Get-ItemProperty -Path $script:realUninstallKeyPath -ErrorAction SilentlyContinue)
  Check "$label-real-install-untouched" ($afterInstall -eq $script:realInstallBefore) "before=$($script:realInstallBefore) after=$afterInstall"
  Check "$label-real-data-untouched" ($afterData -eq $script:realDataBefore) "before=$($script:realDataBefore) after=$afterData"
  Check "$label-real-autostart-untouched" (([bool]$runValue -eq $script:realRunBefore) -and ([bool]$startupValue -eq $script:realStartupBefore)) "runBefore=$($script:realRunBefore) runAfter=$([bool]$runValue) startupBefore=$($script:realStartupBefore) startupAfter=$([bool]$startupValue)"
  Check "$label-real-uninstall-key-untouched" (($null -ne $uninstallKey) -eq $script:realUninstallBefore) "before=$($script:realUninstallBefore) after=$($null -ne $uninstallKey)"
}

try {
  # ---------------------------------------------------------------- safety guards
  $script:realLocalAppData = $env:LOCALAPPDATA
  $realInstall = Join-Path $env:LOCALAPPDATA "Programs\Hangline"
  $realData = Join-Path $env:LOCALAPPDATA "Hangline"
  $realKey = Get-ItemProperty -Path $script:realUninstallKeyPath -ErrorAction SilentlyContinue
  $realRun = Get-ItemProperty -Path $script:realRunKeyPath -Name Hangline -ErrorAction SilentlyContinue
  $realStartup = Get-ItemProperty -Path $script:realStartupKeyPath -Name Hangline -ErrorAction SilentlyContinue
  if ((Test-Path $realInstall) -or $realKey -or $realRun -or $realStartup) {
    throw "A real Hangline installation/autostart entry already exists; refusing to run the lifecycle test (protect user data). Uninstall it first or use a clean VM."
  }
  $script:realInstallBefore = Test-Path $realInstall
  $script:realDataBefore = Test-Path $realData
  $script:realRunBefore = [bool]$realRun
  $script:realStartupBefore = [bool]$realStartup
  $script:realUninstallBefore = ($null -ne $realKey)
  Check "no-preexisting-real-install" (-not (Test-Path $realInstall)) $realInstall
  Check "no-preexisting-uninstall-key" ($null -eq $realKey) "HKCU uninstall key"
  Check "no-preexisting-autostart" ($null -eq $realRun) "HKCU Run Hangline"
  Step "safety guards passed" @{ realDataExists = (Test-Path $realData) }

  # ---------------------------------------------------------------- static production artifact checks
  $setupItem = Get-Item $Setup
  $portableItem = Get-Item $Portable
  $sizeLimit = 15000000
  Check "production-setup-exists" ($null -ne $setupItem) $Setup
  Check "production-setup-size" ($setupItem.Length -le $sizeLimit) @{ bytes = $setupItem.Length; limit = $sizeLimit }
  $setupHash = (Get-FileHash $Setup -Algorithm SHA256).Hash
  $shaFile = Join-Path $repo "release\SHA256SUMS.txt"
  $shaText = if (Test-Path $shaFile) { Get-Content $shaFile -Raw } else { "" }
  Check "production-setup-sha256-listed" ($shaText -match [regex]::Escape($setupHash)) $setupHash
  $vi = (Get-Item $Setup).VersionInfo
  Check "production-setup-version" ($vi.FileVersion -eq $Version) @{ fileVersion = $vi.FileVersion; expected = $Version }
  Check "production-setup-authenticode-notsigned" ((Get-AuthenticodeSignature $Setup).Status -eq "NotSigned") (Get-AuthenticodeSignature $Setup).Status
  $latest = Get-Content (Join-Path $repo "release\latest.json") -Raw | ConvertFrom-Json
  $entry = $latest.platforms."windows-x86_64-nsis"; if (-not $entry) { $entry = $latest.platforms."windows-x86_64" }
  Check "latest.json-version" ($latest.version -eq $Version) $latest.version
  Check "latest.json-signed" ($entry -and $entry.signature -ne "UNSIGNED-PLACEHOLDER" -and $entry.signature.Length -gt 100) @{ signatureChars = if ($entry) { $entry.signature.Length } else { 0 } }
  Check "latest.json-url" ($entry.url -match "Hangline_$([regex]::Escape($Version))_x64-setup\.exe$") $entry.url
  Add-Type -AssemblyName System.IO.Compression.FileSystem
  $zip = [IO.Compression.ZipFile]::OpenRead($Portable)
  try {
    $names = $zip.Entries | ForEach-Object { $_.FullName }
    foreach ($name in @("Hangline.exe", "Hangline.ico", "portable", "README-portable.txt")) {
      Check "portable-contains-$name" ($names -contains $name) $names
    }
    Check "portable-exe-size" (($zip.Entries | Where-Object FullName -eq "Hangline.exe").Length -gt 10000000) "Hangline.exe entry length"
  } finally { $zip.Dispose() }

  # ---------------------------------------------------------------- compile test installer
  Step "compile test-only installer (relocated paths)"
  $tools = Get-Content (Join-Path $repo "src-tauri\target\p5-tools\tools.json") -Raw | ConvertFrom-Json
  $makensis = $tools.nsis.exe
  $webview2 = $tools.webview2.path
  $staging = Join-Path $repo "src-tauri\target\p5-staging"
  if (-not (Test-Path (Join-Path $staging "Hangline.exe"))) { throw "staging exe missing: $staging" }
  if (Test-Path $base) { Remove-Item $base -Recurse -Force }
  New-Item -ItemType Directory -Force (Join-Path $base "StartMenu"), (Join-Path $base "Desktop"), (Join-Path $base "Programs") | Out-Null
  $testSetup = Join-Path $base "Hangline-test-setup.exe"
  $installDir = Join-Path $base "Programs\Hangline"
  $dataDir = Join-Path $base "HanglineData"
  $shortcutDir = Join-Path $base "StartMenu"
  $desktopDir = Join-Path $base "Desktop"
  $nsi = Join-Path $repo "scripts\release\installer.nsi"
  $nsiArgs = @("/V4", "/INPUTCHARSET", "UTF8", "/DOUT_FILE=$testSetup", "/DVERSION=$Version", "/DSOURCE_DIR=$staging", "/DWEBVIEW2_SETUP=$webview2", "/DAPP_INSTALL_DIR=$installDir", "/DAPP_DATA_DIR=$dataDir", "/DSHORTCUT_DIR=$shortcutDir", "/DDESKTOP_SHORTCUT_DIR=$desktopDir", "/DUNINSTALL_KEY=$($script:testUninstallKeyName)", "/DRUN_KEY=$($script:testRunKeyName)", "/DSTARTUP_APPROVED_KEY=$($script:testStartupKeyName)", $nsi)
  $compileLog = Join-Path $base "test-compile.log"
  & $makensis @nsiArgs *> $compileLog
  Check "test-installer-compiled" (Test-Path $testSetup) @{ log = $compileLog }
  if (-not (Test-Path $testSetup)) { throw "test installer compile failed; see $compileLog" }

  # Optional baseline installer for a true version upgrade test (same test paths, older version).
  $oldSetup = ""
  if ($UpgradeFromVersion) {
    $oldSetup = Join-Path $base "Hangline-test-setup-$UpgradeFromVersion.exe"
    # Use the real 0.4.0 payload from the archived portable ZIP when available, so the test is a
    # true old-binary -> new-binary upgrade rather than a same-payload relabel.
    $oldStaging = $staging
    if (Test-Path $BaselinePortable) {
      $oldStaging = Join-Path $base "old-payload"
      New-Item -ItemType Directory -Force $oldStaging | Out-Null
      Expand-Archive -Path $BaselinePortable -DestinationPath $oldStaging -Force
      Copy-Item (Join-Path $staging "Hangline.ico") $oldStaging -Force
      $oldPayloadVersion = (Get-Item (Join-Path $oldStaging "Hangline.exe")).VersionInfo.FileVersion
      Check "baseline-payload-version" ($oldPayloadVersion -eq $UpgradeFromVersion) @{ expected = $UpgradeFromVersion; actual = $oldPayloadVersion; source = (Split-Path $BaselinePortable -Leaf) }
      $script:usedRealOldPayload = $true
    } else {
      Step "baseline portable missing; using current payload for installer-metadata upgrade only" @{ missing = $BaselinePortable }
      $script:usedRealOldPayload = $false
    }
    $oldArgs = @("/V4", "/INPUTCHARSET", "UTF8", "/DOUT_FILE=$oldSetup", "/DVERSION=$UpgradeFromVersion", "/DSOURCE_DIR=$oldStaging", "/DWEBVIEW2_SETUP=$webview2", "/DAPP_INSTALL_DIR=$installDir", "/DAPP_DATA_DIR=$dataDir", "/DSHORTCUT_DIR=$shortcutDir", "/DDESKTOP_SHORTCUT_DIR=$desktopDir", "/DUNINSTALL_KEY=$($script:testUninstallKeyName)", "/DRUN_KEY=$($script:testRunKeyName)", "/DSTARTUP_APPROVED_KEY=$($script:testStartupKeyName)", $nsi)
    & $makensis @oldArgs *>> $compileLog
    Check "baseline-installer-compiled" (Test-Path $oldSetup) @{ version = $UpgradeFromVersion; payload = $oldStaging; log = $compileLog }
  }

  # ---------------------------------------------------------------- install / verify
  if ($oldSetup -and (Test-Path $oldSetup)) {
    Step "install baseline $UpgradeFromVersion (test paths)"
    New-Item -ItemType Directory -Force $dataDir | Out-Null
    Set-Content (Join-Path $dataDir "upgrade-marker.txt") "p5 upgrade keep-data test"
    $pOld = Start-Process -FilePath $oldSetup -ArgumentList "/S" -Wait -PassThru
    Check "baseline-install-exit-code-0" ($pOld.ExitCode -eq 0) $pOld.ExitCode
    $oldKey = Get-RegUninstall
    Check "baseline-install-version" ($oldKey.DisplayVersion -eq $UpgradeFromVersion) @{ expected = $UpgradeFromVersion; actual = $oldKey.DisplayVersion }
    # When the archived 0.4.0 portable is available the real old payload is used and its exe
    # version is asserted; otherwise only the installer metadata upgrade is exercised (see P5-core).
    $baselineExeVersion = (Get-Item (Join-Path $installDir "Hangline.exe")).VersionInfo.FileVersion
    if ($script:usedRealOldPayload) {
      Check "upgrade-source-exe-version" ($baselineExeVersion -eq $UpgradeFromVersion) @{ expected = $UpgradeFromVersion; actual = $baselineExeVersion }
    }
    Step "baseline installer metadata" @{ displayVersion = $oldKey.DisplayVersion; payloadExeVersion = $baselineExeVersion }
  }

  Step "silent install/upgrade to $Version (test paths)"
  $p = Start-Process -FilePath $testSetup -ArgumentList "/S" -Wait -PassThru
  Check "install-exit-code-0" ($p.ExitCode -eq 0) $p.ExitCode
  if ($oldSetup -and (Test-Path $oldSetup)) {
    Check "upgrade-updated-exe-version" ((Get-Item (Join-Path $installDir "Hangline.exe")).VersionInfo.FileVersion -eq $Version) (Get-Item (Join-Path $installDir "Hangline.exe")).VersionInfo.FileVersion
    Check "upgrade-data-kept" (Test-Path (Join-Path $dataDir "upgrade-marker.txt")) $dataDir
  }
  Check "install-exe" (Test-Path (Join-Path $installDir "Hangline.exe")) $installDir
  Check "install-uninstaller" (Test-Path (Join-Path $installDir "uninstall.exe")) $installDir
  Check "install-icon" (Test-Path (Join-Path $installDir "Hangline.ico")) $installDir
  Check "install-start-menu-shortcut" (Test-Path (Join-Path $shortcutDir "Hangline.lnk")) $shortcutDir
  $key = Get-RegUninstall
  Check "install-uninstall-key" ($null -ne $key) "HKCU Uninstall"
  Check "install-uninstall-version" ($key.DisplayVersion -eq $Version) $key.DisplayVersion
  Check "install-uninstall-location" ($key.InstallLocation -eq $installDir) $key.InstallLocation
  Check "install-uninstall-quiet-string" ($key.QuietUninstallString -match "uninstall\.exe") $key.QuietUninstallString
  Check "install-no-real-location" (-not (Test-Path (Join-Path $env:LOCALAPPDATA "Programs\Hangline"))) "real install dir absent"
  Test-RealUntouched "after-install"

  # ---------------------------------------------------------------- launch smoke (isolated app data)
  Step "launch installed app with isolated LOCALAPPDATA"
  $isolatedLocal = Join-Path $base "appdata"
  New-Item -ItemType Directory -Force $isolatedLocal | Out-Null
  $env:LOCALAPPDATA = $isolatedLocal
  $env:APPDATA = (Join-Path $base "appdata\Roaming")
  New-Item -ItemType Directory -Force $env:APPDATA | Out-Null
  $app = Start-Process -FilePath (Join-Path $installDir "Hangline.exe") -PassThru
  Start-Sleep -Seconds 6
  Check "launch-process-alive" (-not $app.HasExited) @{ pid = $app.Id }
  $dataRoot = Join-Path $isolatedLocal "Hangline"
  Check "launch-created-app-data" (Test-Path $dataRoot) $dataRoot
  Check "launch-wrote-settings-or-state" ((Test-Path (Join-Path $dataRoot "settings.json")) -or (Test-Path (Join-Path $dataRoot "state.json"))) (Get-ChildItem $dataRoot -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Name)
  Kill-Tree $app.Id
  Start-Sleep -Seconds 1
  Check "launch-process-stopped" (-not (Get-Process -Id $app.Id -ErrorAction SilentlyContinue)) $app.Id

  # ---------------------------------------------------------------- overwrite / upgrade wiring
  Step "reinstall over the existing install (upgrade wiring)"
  $p2 = Start-Process -FilePath $testSetup -ArgumentList "/S" -Wait -PassThru
  Check "reinstall-exit-code-0" ($p2.ExitCode -eq 0) $p2.ExitCode
  Check "reinstall-version" ((Get-Item (Join-Path $installDir "Hangline.exe")).VersionInfo.FileVersion -eq $Version) (Get-Item (Join-Path $installDir "Hangline.exe")).VersionInfo.FileVersion
  Check "reinstall-data-kept" (Test-Path $dataRoot) $dataRoot

  # ---------------------------------------------------------------- uninstall keep
  Step "silent uninstall with default keep-data"
  New-Item -ItemType Directory -Force $dataDir | Out-Null
  Set-Content (Join-Path $dataDir "keep-marker.txt") "p5 keep-data test"
  Check "keep-test-data-created" (Test-Path (Join-Path $dataDir "keep-marker.txt")) $dataDir
  $env:HANG_UNINSTALL_DATA = $null
  $uninst = Start-Process -FilePath (Join-Path $installDir "uninstall.exe") -ArgumentList "/S" -Wait -PassThru
  Start-Sleep -Seconds 1
  Check "uninstall-keep-exit-code-0" ($uninst.ExitCode -eq 0) $uninst.ExitCode
  Check "uninstall-removed-install-dir" (-not (Test-Path $installDir)) $installDir
  Check "uninstall-removed-shortcut" (-not (Test-Path (Join-Path $shortcutDir "Hangline.lnk"))) $shortcutDir
  Check "uninstall-removed-registry" ($null -eq (Get-RegUninstall)) "HKCU Uninstall"
  Check "uninstall-kept-data" (Test-Path $dataDir) $dataDir
  Test-RealUntouched "after-uninstall-keep"

  # ---------------------------------------------------------------- uninstall delete (test data only)
  Step "install again then uninstall with HANG_UNINSTALL_DATA=delete (test data dir only)"
  $p3 = Start-Process -FilePath $testSetup -ArgumentList "/S" -Wait -PassThru
  Check "reinstall2-exit-code-0" ($p3.ExitCode -eq 0) $p3.ExitCode
  New-Item -ItemType Directory -Force $dataDir | Out-Null
  Set-Content (Join-Path $dataDir "marker.txt") "p5 lifecycle test data"
  $env:HANG_UNINSTALL_DATA = "delete"
  $uninst2 = Start-Process -FilePath (Join-Path $installDir "uninstall.exe") -ArgumentList "/S" -Wait -PassThru
  Start-Sleep -Seconds 1
  Check "uninstall-delete-exit-code-0" ($uninst2.ExitCode -eq 0) $uninst2.ExitCode
  Check "uninstall-delete-removed-data-dir" (-not (Test-Path $dataDir)) $dataDir
  Check "uninstall-delete-removed-registry" ($null -eq (Get-RegUninstall)) "HKCU Uninstall"
  Test-RealUntouched "after-uninstall-delete"

  # ---------------------------------------------------------------- uninstall delete via command line
  Step "install again then uninstall with /S /DELETEUSERDATA (command-line branch, test data only)"
  $p4 = Start-Process -FilePath $testSetup -ArgumentList "/S" -Wait -PassThru
  Check "reinstall3-exit-code-0" ($p4.ExitCode -eq 0) $p4.ExitCode
  New-Item -ItemType Directory -Force $dataDir | Out-Null
  Set-Content (Join-Path $dataDir "cli-marker.txt") "p5 /DELETEUSERDATA test"
  $env:HANG_UNINSTALL_DATA = $null
  $uninst3 = Start-Process -FilePath (Join-Path $installDir "uninstall.exe") -ArgumentList "/S", "/DELETEUSERDATA" -Wait -PassThru
  Start-Sleep -Seconds 1
  Check "uninstall-delete-flag-exit-code-0" ($uninst3.ExitCode -eq 0) $uninst3.ExitCode
  Check "uninstall-delete-flag-removed-data-dir" (-not (Test-Path $dataDir)) $dataDir
  Check "uninstall-delete-flag-removed-registry" ($null -eq (Get-RegUninstall)) "test HKCU Uninstall"
  Test-RealUntouched "after-uninstall-delete-flag"

  # ---------------------------------------------------------------- portable
  Step "portable zip extract + launch"
  $portableDir = Join-Path $base "portable"
  Expand-Archive -Path $Portable -DestinationPath $portableDir -Force
  Check "portable-exe-extracted" (Test-Path (Join-Path $portableDir "Hangline.exe")) $portableDir
  Check "portable-marker-extracted" (Test-Path (Join-Path $portableDir "portable")) $portableDir
  $env:LOCALAPPDATA = Join-Path $base "portable-appdata"
  New-Item -ItemType Directory -Force $env:LOCALAPPDATA | Out-Null
  $app2 = Start-Process -FilePath (Join-Path $portableDir "Hangline.exe") -PassThru
  Start-Sleep -Seconds 6
  Check "portable-launch-alive" (-not $app2.HasExited) @{ pid = $app2.Id }
  Kill-Tree $app2.Id
  Start-Sleep -Seconds 1
  Check "portable-launch-stopped" (-not (Get-Process -Id $app2.Id -ErrorAction SilentlyContinue)) $app2.Id

  # ---------------------------------------------------------------- final cleanup/guards
  Step "final cleanup"
  Check "no-install-dir-left" (-not (Test-Path $installDir)) $installDir
  Check "no-uninstall-key-left" ($null -eq (Get-RegUninstall)) "HKCU Uninstall"
  Check "no-run-value-left" ($null -eq (Get-ItemProperty -Path $script:realRunKeyPath -Name Hangline -ErrorAction SilentlyContinue)) "real HKCU Run"
  Check "no-real-uninstall-key-left" ($null -eq (Get-ItemProperty -Path $script:realUninstallKeyPath -ErrorAction SilentlyContinue)) "real HKCU Uninstall"
  Check "no-real-startup-value-left" ($null -eq (Get-ItemProperty -Path $script:realStartupKeyPath -Name Hangline -ErrorAction SilentlyContinue)) "real StartupApproved"
  Check "test-uninstall-key-cleaned" ($null -eq (Get-RegUninstall)) "test HKCU Uninstall"
  Check "test-run-key-cleaned" ($null -eq (Get-ItemProperty -Path "HKCU:\Software\Microsoft\Windows\CurrentVersion\RunP5Test" -Name Hangline -ErrorAction SilentlyContinue)) "test HKCU RunP5Test"
  Check "test-startup-key-cleaned" ($null -eq (Get-ItemProperty -Path "HKCU:\Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\RunP5Test" -Name Hangline -ErrorAction SilentlyContinue)) "test HKCU StartupApproved RunP5Test"
  $leftover = Get-Process -Name hangline -ErrorAction SilentlyContinue | Where-Object { $_.Path -like "$base*" }
  Check "no-leftover-test-processes" ($null -eq $leftover) ($leftover | Select-Object Id, Path)
  if (-not $KeepBase) {
    Remove-Item $base -Recurse -Force -ErrorAction SilentlyContinue
  }
}
catch {
  $results.errors += $_.Exception.Message
  Check "lifecycle-no-exception" $false $_.Exception.Message
  Write-Host "ERROR: $($_.Exception.Message)" -ForegroundColor Red
}
finally {
  # Restore the caller's environment (the script runs in its own pwsh process, but keep it tidy).
  foreach ($name in @("HANG_UNINSTALL_DATA", "APPDATA")) {
    if (Test-Path "Env:$name") { Remove-Item "Env:$name" -ErrorAction SilentlyContinue }
  }
  # Defensive cleanup of the test-only registry locations even when the run threw mid-way.
  Remove-Item -Path $script:testUninstallKeyPath -Recurse -Force -ErrorAction SilentlyContinue
  Remove-ItemProperty -Path "HKCU:\Software\Microsoft\Windows\CurrentVersion\RunP5Test" -Name Hangline -ErrorAction SilentlyContinue
  Remove-ItemProperty -Path "HKCU:\Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\RunP5Test" -Name Hangline -ErrorAction SilentlyContinue
  $results.finishedAt = (Get-Date).ToString("o")
  $results.failures = $failures
  $results.verdict = if ($failures -eq 0) { "PASS" } else { "FAIL" }
  $jsonPath = Join-Path $evidenceDir "lifecycle.json"
  $results | ConvertTo-Json -Depth 8 | Set-Content $jsonPath -Encoding UTF8
  Write-Host ""
  Write-Host "verdict: $($results.verdict) ($failures failures)"
  Write-Host "evidence: $jsonPath"
  if ($failures -ne 0) { exit 1 }
}
