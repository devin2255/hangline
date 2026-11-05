<#
.SYNOPSIS
  Fetch and cache the packaging tools used by scripts/release/build-release.mjs.

.DESCRIPTION
  Downloads the official NSIS 3.13 zip (the same archive Tauri's bundler uses) and the
  Microsoft Edge WebView2 Evergreen bootstrapper into src-tauri/target/p5-tools/ (git-ignored),
  then verifies that makensis reports v3.13.

  The cache is the primary path: a normal run never depends on a machine-wide NSIS install.
  -UseSystemNSIS reuses %LOCALAPPDATA%\tauri\nsis-3.13 when the cache is empty (useful on
  CI images that already have Tauri's copy) but does not replace the cache.

.PARAMETER UseSystemNSIS
  If the local cache is empty and %LOCALAPPDATA%\tauri\nsis-3.13\Bin\makensis.exe exists,
  use that copy instead of downloading the NSIS archive.

.PARAMETER Force
  Re-download and re-extract both tools even when the cached copies look usable.

.PARAMETER Retries
  Download attempts per URL (default 4, i.e. up to 4 tries).

.PARAMETER TimeoutSec
  Per-attempt HTTP timeout in seconds (default 180).

.NOTES
  Writes src-tauri/target/p5-tools/tools.json with the resolved paths, sizes and SHA-256
  hashes so build-release.mjs can consume the exact tool versions without parsing console
  output. Exit code is non-zero on any download/extraction/verification failure.
#>
[CmdletBinding()]
param(
    [switch]$UseSystemNSIS,
    [switch]$Force,
    [int]$Retries = 4,
    [int]$TimeoutSec = 180
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

# Windows PowerShell 5.1 needs an explicit TLS 1.2 for GitHub/microsoft.com endpoints.
try {
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
} catch {
    # PowerShell 7 / .NET Core: the system default already negotiates TLS 1.2+.
}

$NsisVersion = '3.13'
$NsisUrl = "https://github.com/tauri-apps/binary-releases/releases/download/nsis-$NsisVersion/nsis-$NsisVersion.zip"
$WebView2Url = 'https://go.microsoft.com/fwlink/p/?LinkId=2124703'

$scriptDir = if ($PSScriptRoot) { $PSScriptRoot } else { Split-Path -Parent $MyInvocation.MyCommand.Path }
$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $scriptDir '..\..'))
$toolsDir = Join-Path $repoRoot 'src-tauri\target\p5-tools'
$nsisDir = Join-Path $toolsDir "nsis-$NsisVersion"
$nsisExe = Join-Path $nsisDir 'Bin\makensis.exe'
$nsisZip = Join-Path $toolsDir "nsis-$NsisVersion.zip"
$webView2Exe = Join-Path $toolsDir 'MicrosoftEdgeWebView2Setup.exe'
$toolsJson = Join-Path $toolsDir 'tools.json'

$MinNsisZipBytes = 2MB        # actual archive is ~2.36 MB
$MinWebView2Bytes = 1MB       # actual bootstrapper is ~1.9 MB

function Write-Step {
    param([string]$Message)
    Write-Host "[fetch-tools] $Message"
}

function Invoke-DownloadWithRetry {
    param(
        [Parameter(Mandatory = $true)][string]$Uri,
        [Parameter(Mandatory = $true)][string]$Destination,
        [Parameter(Mandatory = $true)][long]$MinBytes,
        [Parameter(Mandatory = $true)][int]$Attempts,
        [Parameter(Mandatory = $true)][int]$Timeout
    )

    for ($attempt = 1; $attempt -le $Attempts; $attempt++) {
        try {
            if (Test-Path -LiteralPath $Destination) {
                Remove-Item -LiteralPath $Destination -Force
            }
            Write-Step "Downloading $Uri (attempt $attempt/$Attempts)"
            Invoke-WebRequest -Uri $Uri -OutFile $Destination -UseBasicParsing -TimeoutSec $Timeout

            $bytes = (Get-Item -LiteralPath $Destination).Length
            if ($bytes -lt $MinBytes) {
                throw "downloaded file is too small: $bytes bytes (expected at least $MinBytes)"
            }
            Write-Step "Downloaded $([System.IO.Path]::GetFileName($Destination)) ($bytes bytes)"
            return
        } catch {
            $message = $_.Exception.Message
            Write-Warning "Download attempt $attempt/$Attempts failed: $message"
            if (Test-Path -LiteralPath $Destination) {
                Remove-Item -LiteralPath $Destination -Force -ErrorAction SilentlyContinue
            }
            if ($attempt -eq $Attempts) {
                throw "download failed after $Attempts attempt(s): $Uri :: $message"
            }
            $delay = [Math]::Min(30, 2 * $attempt)
            Write-Step "Retrying in $delay seconds..."
            Start-Sleep -Seconds $delay
        }
    }
}

function Expand-NsisArchive {
    param(
        [Parameter(Mandatory = $true)][string]$ZipPath,
        [Parameter(Mandatory = $true)][string]$Destination
    )

    $tempDir = Join-Path ([System.IO.Path]::GetTempPath()) ("hangline-nsis-" + [Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $tempDir -Force | Out-Null
    try {
        Write-Step "Extracting $([System.IO.Path]::GetFileName($ZipPath)) with Expand-Archive"
        Expand-Archive -LiteralPath $ZipPath -DestinationPath $tempDir -Force

        # The archive is expected to contain nsis-3.13/Bin/makensis.exe; tolerate a
        # flat layout too so a future repack does not break the release script.
        $sourceRoot = $null
        if (Test-Path -LiteralPath (Join-Path $tempDir 'Bin\makensis.exe')) {
            $sourceRoot = $tempDir
        } else {
            $candidate = Get-ChildItem -LiteralPath $tempDir -Directory |
                Where-Object { Test-Path -LiteralPath (Join-Path $_.FullName 'Bin\makensis.exe') } |
                Select-Object -First 1
            if ($candidate) {
                $sourceRoot = $candidate.FullName
            }
        }
        if (-not $sourceRoot) {
            throw "unexpected NSIS archive layout: Bin\makensis.exe not found after extraction"
        }

        if (Test-Path -LiteralPath $Destination) {
            # Guard the recursive delete: only ever remove the cache subdirectory itself.
            $resolvedDestination = [System.IO.Path]::GetFullPath($Destination)
            if (-not $resolvedDestination.StartsWith($toolsDir, [System.StringComparison]::OrdinalIgnoreCase)) {
                throw "refusing to remove unexpected path: $resolvedDestination"
            }
            Remove-Item -LiteralPath $Destination -Recurse -Force
        }
        New-Item -ItemType Directory -Path $Destination -Force | Out-Null
        Copy-Item -Path (Join-Path $sourceRoot '*') -Destination $Destination -Recurse -Force
        Write-Step "Extracted NSIS $NsisVersion into $Destination"
    } finally {
        Remove-Item -LiteralPath $tempDir -Recurse -Force -ErrorAction SilentlyContinue
    }
}

function Test-NsisVersion {
    param([Parameter(Mandatory = $true)][string]$Makensis)

    if (-not (Test-Path -LiteralPath $Makensis)) {
        throw "makensis not found at $Makensis"
    }
    # makensis /VERSION prints e.g. "v3.13" without a trailing newline.
    $output = (& $Makensis /VERSION 2>&1 | Out-String).Trim()
    $exitCode = $LASTEXITCODE
    if ($exitCode -ne 0) {
        throw "makensis /VERSION failed with exit code ${exitCode}: $output"
    }
    if ($output -notmatch ([regex]::Escape($NsisVersion))) {
        throw "unexpected NSIS version: '$output' (expected $NsisVersion)"
    }
    Write-Step "makensis /VERSION -> $output"
    return $output
}

New-Item -ItemType Directory -Path $toolsDir -Force | Out-Null
Write-Step "Repository root: $repoRoot"
Write-Step "Tools cache: $toolsDir"

# ---------------------------------------------------------------- NSIS 3.13
$nsisSource = $null
$nsisVersionOutput = $null

if ((-not $Force) -and (Test-Path -LiteralPath $nsisExe)) {
    $nsisSource = 'cache'
    $nsisVersionOutput = Test-NsisVersion -Makensis $nsisExe
} elseif ($UseSystemNSIS) {
    $systemNsisDir = Join-Path $env:LOCALAPPDATA "tauri\nsis-$NsisVersion"
    $systemNsisExe = Join-Path $systemNsisDir 'Bin\makensis.exe'
    if (Test-Path -LiteralPath $systemNsisExe) {
        Write-Step "Using system NSIS cache at $systemNsisDir (requested by -UseSystemNSIS)"
        $nsisExe = $systemNsisExe
        $nsisSource = 'system'
        $nsisVersionOutput = Test-NsisVersion -Makensis $nsisExe
    } else {
        Write-Warning "-UseSystemNSIS was set but $systemNsisExe does not exist; downloading the official archive."
    }
}

if (-not $nsisSource) {
    if ($Force -or -not (Test-Path -LiteralPath $nsisZip) -or ((Get-Item -LiteralPath $nsisZip).Length -lt $MinNsisZipBytes)) {
        Invoke-DownloadWithRetry -Uri $NsisUrl -Destination $nsisZip -MinBytes $MinNsisZipBytes -Attempts $Retries -Timeout $TimeoutSec
    } else {
        Write-Step "Reusing cached NSIS archive ($((Get-Item -LiteralPath $nsisZip).Length) bytes)"
    }

    Expand-NsisArchive -ZipPath $nsisZip -Destination $nsisDir
    $nsisExe = Join-Path $nsisDir 'Bin\makensis.exe'
    $nsisSource = 'cache'
    $nsisVersionOutput = Test-NsisVersion -Makensis $nsisExe
}

# --------------------------------------------------- WebView2 bootstrapper
if ($Force -or -not (Test-Path -LiteralPath $webView2Exe) -or ((Get-Item -LiteralPath $webView2Exe).Length -lt $MinWebView2Bytes)) {
    Invoke-DownloadWithRetry -Uri $WebView2Url -Destination $webView2Exe -MinBytes $MinWebView2Bytes -Attempts $Retries -Timeout $TimeoutSec
} else {
    Write-Step "Reusing cached WebView2 bootstrapper ($((Get-Item -LiteralPath $webView2Exe).Length) bytes)"
}

# ------------------------------------------------------------- summary
$nsisHash = (Get-FileHash -LiteralPath $nsisExe -Algorithm SHA256).Hash
$webView2Item = Get-Item -LiteralPath $webView2Exe
$webView2Hash = (Get-FileHash -LiteralPath $webView2Exe -Algorithm SHA256).Hash
$nsisItem = Get-Item -LiteralPath $nsisExe

$result = [ordered]@{
    generated_at = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
    nsis         = [ordered]@{
        version    = $NsisVersion
        version_raw = $nsisVersionOutput
        source     = $nsisSource
        exe        = $nsisExe
        bytes      = $nsisItem.Length
        sha256     = $nsisHash
    }
    webview2     = [ordered]@{
        path   = $webView2Exe
        bytes  = $webView2Item.Length
        sha256 = $webView2Hash
    }
}

$json = $result | ConvertTo-Json -Depth 5
[System.IO.File]::WriteAllText($toolsJson, $json, (New-Object System.Text.UTF8Encoding($false)))

Write-Step "NSIS $NsisVersion ready: $nsisExe"
Write-Step "  sha256 $nsisHash"
Write-Step "WebView2 bootstrapper ready: $webView2Exe"
Write-Step "  $($webView2Item.Length) bytes, sha256 $webView2Hash"
Write-Step "Wrote $toolsJson"
