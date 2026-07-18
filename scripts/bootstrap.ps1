# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
#
# Bootstrap installer for Windows. Downloads the latest Meridian NSIS installer
# from the GitHub release and runs it. The app stages its own background daemon
# on first run and opens the setup wizard (AI provider + tracker) - there is no
# Node or Python to install; the package is self-contained.
#
# One-liner usage:
#   irm https://raw.githubusercontent.com/Meridiona/meridian/main/scripts/bootstrap.ps1 | iex
#
# Or run directly from a clone:
#   powershell -ExecutionPolicy Bypass -File scripts\bootstrap.ps1
#
# Re-running is safe - it reinstalls the latest build over any existing one.
#
# This is the Windows counterpart of scripts/bootstrap.sh. Kept as a separate
# file rather than a branch inside that script: the two share no mechanics
# (DMG mount + drag-to-Applications + quarantine strip vs. running an
# installer), and a single script trying to do both would be harder to read
# than either alone.

$ErrorActionPreference = 'Stop'

function Write-Step($msg) { Write-Host "-> $msg" }
function Write-Ok($msg)   { Write-Host "   [ok] $msg" }
function Write-Fail($msg) { Write-Host "[x] $msg" -ForegroundColor Red; exit 1 }

# -- 0. Platform guard -------------------------------------------------------
# The NSIS package is x64. Windows on ARM runs it under emulation, which works
# but is worth saying out loud rather than letting the user wonder.
if (-not $IsWindows -and $PSVersionTable.PSVersion.Major -ge 6) {
    Write-Fail "This installer is for Windows. On macOS use scripts/bootstrap.sh."
}
if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') {
    Write-Host "   [!] Windows on ARM detected - Meridian runs under x64 emulation." -ForegroundColor Yellow
}

$InstallerUrl = 'https://github.com/Meridiona/meridian/releases/latest/download/Meridian-setup.exe'
$TempDir = Join-Path $env:TEMP "meridian-install-$(Get-Random)"
New-Item -ItemType Directory -Path $TempDir -Force | Out-Null

try {
    # -- 1. Download the installer -------------------------------------------
    $Installer = Join-Path $TempDir 'Meridian-setup.exe'
    Write-Step 'Downloading the latest Meridian release...'
    try {
        # Explicit TLS 1.2 for Windows PowerShell 5.1, whose default protocol
        # set predates it and fails against GitHub.
        [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
        Invoke-WebRequest -Uri $InstallerUrl -OutFile $Installer -UseBasicParsing
    } catch {
        Write-Fail @"
Download failed from $InstallerUrl
    $($_.Exception.Message)
    Check your connection, or download the installer manually from:
    https://github.com/Meridiona/meridian/releases/latest
"@
    }
    $SizeMb = [math]::Round((Get-Item $Installer).Length / 1MB, 1)
    Write-Ok "downloaded ($SizeMb MB)"

    # -- 2. Stop a running instance so the installer can replace its files ----
    Get-Process -Name 'meridian-tray' -ErrorAction SilentlyContinue |
        Stop-Process -Force -ErrorAction SilentlyContinue
    Start-Sleep -Seconds 1

    # -- 3. Run it -----------------------------------------------------------
    # /S is NSIS's silent switch. The package installs per-user (installMode
    # currentUser in tauri.windows.conf.json), so no elevation is needed and no
    # UAC prompt appears.
    Write-Step 'Installing Meridian...'
    $proc = Start-Process -FilePath $Installer -ArgumentList '/S' -Wait -PassThru
    if ($proc.ExitCode -ne 0) {
        Write-Fail "Installer exited with code $($proc.ExitCode)."
    }
    Write-Ok 'installed'
} finally {
    Remove-Item -Recurse -Force $TempDir -ErrorAction SilentlyContinue
}

Write-Host ''
Write-Host '======================================================================'
Write-Host '  Meridian is installed and starting in your system tray.'
Write-Host ''
Write-Host '  The setup wizard opens on first launch - it walks you through:'
Write-Host '    - the AI that writes your summaries (a coding-agent CLI, or a cloud endpoint)'
Write-Host '    - connecting your issue tracker (Jira / Linear / GitHub / Trello / Azure DevOps)'
Write-Host ''
Write-Host '  Updates install themselves from within the app - no need to re-run this.'
Write-Host '======================================================================'
