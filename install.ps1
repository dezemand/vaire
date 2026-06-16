<#
.SYNOPSIS
    Vairë installer for Windows.

.DESCRIPTION
    Downloads a prebuilt vaire.exe from the latest GitHub Release and installs it
    into a bin directory, adding that directory to your user PATH if needed.

.EXAMPLE
    irm https://raw.githubusercontent.com/dezemand/vaire/main/install.ps1 | iex

.PARAMETER Version
    Tag to install (e.g. v0.1.0). Defaults to the latest release.
    Override via the env var VAIRE_VERSION.

.PARAMETER InstallDir
    Where to put vaire.exe. Defaults to %LOCALAPPDATA%\Programs\vaire.
    Override via the env var VAIRE_INSTALL_DIR.
#>
[CmdletBinding()]
param(
    [string]$Version    = $env:VAIRE_VERSION,
    [string]$InstallDir = $env:VAIRE_INSTALL_DIR
)

$ErrorActionPreference = 'Stop'
$Repo = 'dezemand/vaire'
$Bin  = 'vaire'

# --- detect architecture -----------------------------------------------------
$arch = $env:PROCESSOR_ARCHITECTURE
switch ($arch) {
    'AMD64' { $target = 'x86_64-pc-windows-msvc' }
    default {
        throw "No prebuilt binary for architecture '$arch'. Build from source with: cargo install --path ."
    }
}

# --- resolve version ---------------------------------------------------------
if (-not $Version) {
    Write-Host 'Resolving latest release...' -ForegroundColor DarkGray
    $rel = Invoke-RestMethod "https://api.github.com/repos/$Repo/releases/latest" `
        -Headers @{ 'User-Agent' = 'vaire-installer' }
    $Version = $rel.tag_name
    if (-not $Version) { throw 'Could not determine the latest release version. Set VAIRE_VERSION.' }
}

$stem  = "$Bin-$Version-$target"
$asset = "$stem.zip"
$url   = "https://github.com/$Repo/releases/download/$Version/$asset"

# --- install dir -------------------------------------------------------------
if (-not $InstallDir) {
    $InstallDir = Join-Path $env:LOCALAPPDATA "Programs\vaire"
}

Write-Host "Installing $Bin $Version " -NoNewline
Write-Host "($target)" -ForegroundColor DarkGray
Write-Host "  from $url" -ForegroundColor DarkGray
Write-Host "  to   $InstallDir" -ForegroundColor DarkGray

# --- download + extract ------------------------------------------------------
$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("vaire-" + [System.Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $tmp -Force | Out-Null
try {
    $zip = Join-Path $tmp $asset
    Invoke-WebRequest -Uri $url -OutFile $zip -UseBasicParsing -Headers @{ 'User-Agent' = 'vaire-installer' }
    Expand-Archive -Path $zip -DestinationPath $tmp -Force

    # Archive holds a top-level directory ($stem) with the .exe; fall back to flat.
    $src = Join-Path $tmp "$stem\$Bin.exe"
    if (-not (Test-Path $src)) { $src = Join-Path $tmp "$Bin.exe" }
    if (-not (Test-Path $src)) { throw "Binary not found in archive." }

    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    Copy-Item -Path $src -Destination (Join-Path $InstallDir "$Bin.exe") -Force
}
finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}

Write-Host "Installed $InstallDir\$Bin.exe" -ForegroundColor Green

# --- ensure on PATH (user scope) ---------------------------------------------
$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
$onPath = ($userPath -split ';') -contains $InstallDir
if (-not $onPath) {
    $newPath = if ([string]::IsNullOrEmpty($userPath)) { $InstallDir } else { "$userPath;$InstallDir" }
    [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
    $env:Path = "$env:Path;$InstallDir"   # make it usable in the current session too
    Write-Host "Added $InstallDir to your user PATH. Restart your terminal for it to take effect everywhere." -ForegroundColor Yellow
}

Write-Host "Run `"$Bin --help`" to get started."
