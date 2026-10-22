# Copyright (C) 2026 Ashwani Kumar
# SPDX-License-Identifier: GPL-3.0-or-later

<#
.SYNOPSIS
  Builds PeekABoo Notes and its Inno Setup installer
  (target\setup\PeekABooNotes-Setup-<version>.exe).

.DESCRIPTION
  Needs Inno Setup 6 (winget install JRSoftware.InnoSetup). Run with:
    powershell -ExecutionPolicy Bypass -File packaging\inno\build-setup.ps1 [-NoBuild]
#>
param(
    # Use the exe already in target\release instead of building it first.
    [switch]$NoBuild
)
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path

$iscc = @(
    (Join-Path $env:LOCALAPPDATA 'Programs\Inno Setup 6\ISCC.exe'),
    (Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 6\ISCC.exe'),
    (Join-Path $env:ProgramFiles 'Inno Setup 6\ISCC.exe')
) | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $iscc) { throw 'Inno Setup 6 not found (winget install JRSoftware.InnoSetup)' }

$version = (Select-String -Path (Join-Path $root 'Cargo.toml') -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1).Matches[0].Groups[1].Value

if (-not $NoBuild) {
    Push-Location $root
    try { cargo build --release; if ($LASTEXITCODE) { throw 'cargo build failed' } } finally { Pop-Location }
}

& $iscc "/DAppVersion=$version" "/DSourceDir=$root\target\release" "/DOutputDir=$root\target\setup" (Join-Path $PSScriptRoot 'PeekABooNotes.iss')
if ($LASTEXITCODE) { throw 'Inno Setup failed' }
Write-Host "setup: $root\target\setup\PeekABooNotes-Setup-$version.exe"
