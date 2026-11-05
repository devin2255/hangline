#!/usr/bin/env pwsh
<#
.SYNOPSIS
  Generates the neutral P3-5 recording fixture (3 s, 640x360, H.264 + 440 Hz AAC mp4) with the
  installed ffmpeg.

.DESCRIPTION
  Dev-only helper; the generated mp4 is never committed. The default output lives under
  `src-tauri/target/p3-fixtures/` (git-ignored) so it can also be passed to the ignored Rust test:

    $env:HANG_TEST_VIDEO = "<repo>\src-tauri\target\p3-fixtures\p3-recording-fixture.mp4"
    cargo test video:: -- --ignored --nocapture

  The short sine track exists so the trim E2E can prove the WinRT export preserves audio (the
  P3-6 review follow-up); a video-only fixture cannot distinguish "audio copied" from "silent
  track synthesized by the media runtime".

  Exact command (one line, from the repository root):

    ffmpeg -y -v error -f lavfi -i "color=c=0x1E6F73:s=640x360:r=30:d=3" -f lavfi -i "sine=frequency=440:sample_rate=48000:duration=3" -vf "drawgrid=width=80:height=60:thickness=2:color=white@0.22" -c:v libx264 -preset veryfast -pix_fmt yuv420p -c:a aac -b:a 96k -shortest -movflags +faststart src-tauri/target/p3-fixtures/p3-recording-fixture.mp4

.PARAMETER Out
  Output path; defaults to src-tauri/target/p3-fixtures/p3-recording-fixture.mp4.
#>
param(
  [string]$Out = (Join-Path $PSScriptRoot "..\..\src-tauri\target\p3-fixtures\p3-recording-fixture.mp4")
)

$ErrorActionPreference = "Stop"

$ffmpeg = (Get-Command ffmpeg -ErrorAction Stop).Source
$ffprobe = (Get-Command ffprobe -ErrorAction Stop).Source
$Out = [System.IO.Path]::GetFullPath($Out)
$parent = Split-Path -Parent $Out
New-Item -ItemType Directory -Force $parent | Out-Null

# A flat teal frame with a light grid: neutral synthetic art (no desktop, no user content) whose
# first-frame pixels are predictable for the evidence script. The 440 Hz sine is inaudible in the
# evidence run (nothing plays it) but gives the export a real audio stream to preserve.
$video = "color=c=0x1E6F73:s=640x360:r=30:d=3"
$audio = "sine=frequency=440:sample_rate=48000:duration=3"
$filter = "drawgrid=width=80:height=60:thickness=2:color=white@0.22"

& $ffmpeg -y -v error -f lavfi -i $video -f lavfi -i $audio -vf $filter -c:v libx264 -preset veryfast -pix_fmt yuv420p -c:a aac -b:a 96k -shortest -movflags +faststart $Out
if ($LASTEXITCODE -ne 0) {
  throw "ffmpeg failed with exit code $LASTEXITCODE"
}

$probe = & $ffprobe -v error -show_entries "stream=codec_type,codec_name,width,height:format=duration" -of json $Out | ConvertFrom-Json
$stream = $probe.streams | Where-Object { $_.codec_type -eq "video" } | Select-Object -First 1
$audioStream = $probe.streams | Where-Object { $_.codec_type -eq "audio" } | Select-Object -First 1
$duration = [double]$probe.format.duration
if ($stream.width -ne 640 -or $stream.height -ne 360) {
  throw "unexpected fixture dimensions: $($stream.width)x$($stream.height)"
}
if (-not $audioStream) {
  throw "the fixture must contain an audio stream (trim audio-preservation evidence)"
}
if ([math]::Abs($duration - 3.0) -gt 0.1) {
  throw "unexpected fixture duration: $duration s"
}

Write-Host "fixture-video: $Out"
Write-Host "fixture-video: $($stream.width)x$($stream.height), $([math]::Round($duration, 3)) s, H.264 + $($audioStream.codec_name) audio (ffmpeg 8.x)"
