<#
.SYNOPSIS
  Builds Peekaboo Notes and packs it as an MSIX (target\msix\PeekabooNotes_<ver>_x64.msix),
  optionally signed.

.DESCRIPTION
  Script execution is usually disabled, so run it as:

    powershell -ExecutionPolicy Bypass -File packaging\build-msix.ps1
    powershell -ExecutionPolicy Bypass -File packaging\build-msix.ps1 -CertPath dev.pfx -Password secret
    powershell -ExecutionPolicy Bypass -File packaging\build-msix.ps1 -Thumbprint 0123ABCD...

  Steps: cargo build --release; stage the exe, lang\, Assets\ and AppxManifest.xml (version
  taken from Cargo.toml) in target\msix\layout; index the logos into resources.pri
  (makepri); makeappx pack; sign when a certificate is given. The certificate's subject
  must equal the manifest's Publisher (default "CN=Peekaboo Notes Dev", see -Publisher).

  Windows only installs signed packages from a trusted publisher. Without a certificate,
  install the staged layout instead (Developer Mode on):
    Add-AppxPackage -Register target\msix\layout\AppxManifest.xml

  Never stage a `portable` file: the package folder is read-only, notes must go to
  Documents\Peekaboo Notes.

.PARAMETER NoBuild
  Skip cargo build and package the exe already in -ExeDir.
.PARAMETER ExeDir
  Folder holding peekaboo-notes.exe and lang\ (default: target\release, or
  $env:CARGO_TARGET_DIR\release).
.PARAMETER SdkBin
  Folder with makeappx.exe, makepri.exe, signtool.exe (default: newest Windows 10/11 SDK, x64).
#>
param(
    [switch]$NoBuild,
    [string]$ExeDir,
    [string]$Publisher = 'CN=Peekaboo Notes Dev',
    [string]$CertPath,
    [string]$Password,
    [string]$Thumbprint,
    [string]$TimestampUrl,
    [string]$SdkBin
)

$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$src = Join-Path $PSScriptRoot 'msix'

function Fail($msg) { Write-Host "error: $msg" -ForegroundColor Red; exit 1 }
function Run($exe, [string[]]$argv, [switch]$Quiet) {
    if ($Quiet) { $log = & $exe @argv } else { & $exe @argv }
    if ($LASTEXITCODE -ne 0) { $log; Fail "$(Split-Path $exe -Leaf) failed ($LASTEXITCODE)" }
}

# Windows SDK tools: the newest bin\<version>\x64 that has makeappx.
if (-not $SdkBin) {
    $kits = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10\bin'
    $found = Get-ChildItem $kits -Directory -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -match '^10\.' -and (Test-Path (Join-Path $_.FullName 'x64\makeappx.exe')) } |
        Sort-Object { [version]$_.Name } -Descending | Select-Object -First 1
    if (-not $found) { Fail "Windows SDK not found under $kits (install it or pass -SdkBin)" }
    $SdkBin = Join-Path $found.FullName 'x64'
}
$makeappx = Join-Path $SdkBin 'makeappx.exe'
$makepri = Join-Path $SdkBin 'makepri.exe'
$signtool = Join-Path $SdkBin 'signtool.exe'

# Version: Cargo's x.y.z -> MSIX x.y.z.0.
$cargo = [IO.File]::ReadAllText((Join-Path $root 'Cargo.toml'))
if ($cargo -notmatch '(?m)^version\s*=\s*"(\d+)\.(\d+)\.(\d+)') { Fail 'no version in Cargo.toml' }
$version = "$($Matches[1]).$($Matches[2]).$($Matches[3]).0"

if (-not $ExeDir) {
    if ($env:CARGO_TARGET_DIR) { $ExeDir = Join-Path $env:CARGO_TARGET_DIR 'release' }
    else { $ExeDir = Join-Path $root 'target\release' }
}
if (-not $NoBuild) {
    Push-Location $root
    try { Run 'cargo' @('build', '--release') } finally { Pop-Location }
}
$exe = Join-Path $ExeDir 'peekaboo-notes.exe'
if (-not (Test-Path $exe)) { Fail "$exe not found" }

# Stage the layout.
$out = Join-Path $root 'target\msix'
$layout = Join-Path $out 'layout'
$priSrc = Join-Path $out 'pri'
foreach ($d in @($layout, $priSrc)) {
    if (Test-Path $d) { Remove-Item $d -Recurse -Force }
    New-Item -ItemType Directory $d | Out-Null
}
Copy-Item $exe $layout
$lang = Join-Path $ExeDir 'lang'
if (-not (Test-Path $lang)) { $lang = Join-Path $root 'src\i18n\strings' }
New-Item -ItemType Directory (Join-Path $layout 'lang') | Out-Null
Copy-Item (Join-Path $lang 'strings.*.xml') (Join-Path $layout 'lang')
Copy-Item (Join-Path $src 'Assets') $layout -Recurse

$manifest = [IO.File]::ReadAllText((Join-Path $src 'AppxManifest.xml'))
$manifest = $manifest -replace '(<Identity[^>]*?\sVersion=")[^"]*', "`${1}$version"
$manifest = $manifest -replace '(<Identity[^>]*?\sPublisher=")[^"]*', "`${1}$([Security.SecurityElement]::Escape($Publisher))"
$utf8 = New-Object Text.UTF8Encoding($false)
[IO.File]::WriteAllText((Join-Path $layout 'AppxManifest.xml'), $manifest, $utf8)

# resources.pri, so the scale-/targetsize- logos resolve. Only the logos are indexed (the
# app reads lang\ itself), from a copy holding just them and the manifest.
Copy-Item (Join-Path $src 'Assets') $priSrc -Recurse
Copy-Item (Join-Path $layout 'AppxManifest.xml') $priSrc
$priCfg = Join-Path $out 'priconfig.xml'
Run $makepri @('createconfig', '/cf', $priCfg, '/dq', 'en-US', '/pv', '10.0.0', '/o') -Quiet
# One .pri for one package: drop the split into per-scale resource packs (for bundles).
$cfg = [xml][IO.File]::ReadAllText($priCfg)
foreach ($p in @($cfg.SelectNodes('//packaging'))) { [void]$p.ParentNode.RemoveChild($p) }
$cfg.Save($priCfg)
Run $makepri @('new', '/pr', $priSrc, '/cf', $priCfg, '/mn', (Join-Path $priSrc 'AppxManifest.xml'), '/of', (Join-Path $layout 'resources.pri'), '/o') -Quiet
Remove-Item $priSrc -Recurse -Force

# Pack.
$msix = Join-Path $out "PeekabooNotes_${version}_x64.msix"
Run $makeappx @('pack', '/d', $layout, '/p', $msix, '/o')

# Sign (optional).
if ($CertPath -or $Thumbprint) {
    $sign = @('sign', '/fd', 'SHA256')
    if ($CertPath) {
        $sign += @('/f', (Resolve-Path $CertPath).Path)
        if ($Password) { $sign += @('/p', $Password) }
    } else {
        $sign += @('/sha1', $Thumbprint)
    }
    if ($TimestampUrl) { $sign += @('/tr', $TimestampUrl, '/td', 'SHA256') }
    Run $signtool ($sign + @($msix))
    Write-Host "signed: $msix"
} else {
    Write-Host "unsigned: $msix (pass -CertPath or -Thumbprint to sign)"
}
