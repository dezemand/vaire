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

    # Verify against the release's published digests before extracting. HTTPS authenticates
    # the transport, not the artifact, so it is no defence against a replaced release asset.
    # Set VAIRE_SKIP_CHECKSUM=1 to bypass deliberately.
    if ($env:VAIRE_SKIP_CHECKSUM -eq '1') {
        Write-Host "  skipping checksum verification (VAIRE_SKIP_CHECKSUM=1)" -ForegroundColor DarkGray
    }
    else {
        $sumsUrl = "https://github.com/$Repo/releases/download/$Version/SHA256SUMS"
        $sums = Join-Path $tmp 'SHA256SUMS'
        $havesums = $true
        try {
            Invoke-WebRequest -Uri $sumsUrl -OutFile $sums -UseBasicParsing -Headers @{ 'User-Agent' = 'vaire-installer' }
        }
        catch {
            # Releases published before SHA256SUMS existed have nothing to verify against.
            $havesums = $false
            Write-Host "  no SHA256SUMS published for $Version - skipping verification" -ForegroundColor DarkGray
        }
        if ($havesums) {
            # Exact filename match (stripping sha256sum's binary-mode '*' marker), not a
            # substring search — a sibling asset like "<asset>.sig" would also match.
            $expected = $null
            foreach ($line in Get-Content $sums) {
                $fields = $line -split '\s+', 2
                if ($fields.Count -eq 2 -and $fields[1].Trim().TrimStart('*') -eq $asset) {
                    $expected = $fields[0]
                    break
                }
            }
            if (-not $expected) { throw "No checksum for $asset in SHA256SUMS." }
            $actual = (Get-FileHash -Path $zip -Algorithm SHA256).Hash
            if ($actual -ne $expected) {
                throw "Checksum mismatch for ${asset}: expected $expected, got $actual. Refusing to install."
            }
            Write-Host "  checksum ok" -ForegroundColor DarkGray
        }
    }

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
# Read and write the raw registry value rather than going through
# [Environment]::GetEnvironmentVariable/SetEnvironmentVariable. Those expand and rewrite the
# value as REG_SZ: a user PATH stored as REG_EXPAND_SZ (entries like %USERPROFILE%\bin — a
# common setup) would come back with those references frozen to whatever they resolve to
# today, silently breaking if the profile ever moves. Preserving the existing value kind
# keeps the user's PATH exactly as they wrote it.
$key = 'HKCU:\Environment'
$raw = (Get-ItemProperty -Path $key -Name Path -ErrorAction SilentlyContinue)
$userPath = if ($null -ne $raw) { $raw.Path } else { '' }
$onPath = ($userPath -split ';' | Where-Object { $_ }) -contains $InstallDir
if (-not $onPath) {
    $kind = 'String'
    try {
        $existing = (Get-Item -Path $key).GetValueKind('Path')
        if ($existing -eq 'ExpandString') { $kind = 'ExpandString' }
    }
    catch { }   # no existing Path value: a plain REG_SZ is right
    $newPath = if ([string]::IsNullOrEmpty($userPath)) { $InstallDir } else { "$userPath;$InstallDir" }
    New-ItemProperty -Path $key -Name Path -Value $newPath -PropertyType $kind -Force | Out-Null
    $env:Path = "$env:Path;$InstallDir"   # make it usable in the current session too
    Write-Host "Added $InstallDir to your user PATH. Restart your terminal for it to take effect everywhere." -ForegroundColor Yellow
}

Write-Host "Run `"$Bin --help`" to get started."
