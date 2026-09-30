param(
    [Parameter(Mandatory = $true)]
    [string]$InstallerPath,
    [int]$StartupTimeoutSeconds = 90
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Write-Step([string]$Message) { Write-Host "==> $Message" }
function Assert-True([bool]$Condition, [string]$Message) {
    if (-not $Condition) { throw $Message }
}
function Wait-Until([scriptblock]$Condition, [int]$TimeoutSeconds, [string]$Description) {
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    do {
        if (& $Condition) { return }
        Start-Sleep -Seconds 2
    } while ((Get-Date) -lt $deadline)
    throw "Timed out waiting for $Description after $TimeoutSeconds seconds"
}

$installer = (Resolve-Path $InstallerPath).Path
$installDir = $null
$appExe = $null
$meridianDir = Join-Path $HOME '.meridian'
$daemonExe = Join-Path $meridianDir 'bin\meridian.exe'
$backendMarker = Join-Path $meridianDir 'backend-version'
$startupDir = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\Startup'
$daemonFallback = Join-Path $startupDir 'MeridianDaemon.vbs'
$trayFallback = Join-Path $startupDir 'MeridianTray.vbs'

Write-Step "Pre-cleaning stale Meridian processes"
Get-Process -Name 'Meridian','meridian','meridian-tray' -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue

Write-Step "Installing NSIS package silently"
$install = Start-Process -FilePath $installer -ArgumentList '/S' -Wait -PassThru
Assert-True ($install.ExitCode -eq 0) "NSIS installer exited with code $($install.ExitCode)"

Write-Step "Locating installed application binary"
$candidates = @(
    (Join-Path $env:LOCALAPPDATA 'Meridian\Meridian.exe'),
    (Join-Path $env:LOCALAPPDATA 'Programs\Meridian\Meridian.exe')
)

$deadline = (Get-Date).AddSeconds(30)
do {
    foreach ($candidate in $candidates) {
        if (Test-Path $candidate) {
            $appExe = (Resolve-Path $candidate).Path
            break
        }
    }

    if (-not $appExe) {
        $found = Get-ChildItem -Path $env:LOCALAPPDATA -Filter 'Meridian.exe' -File -Recurse -Depth 4 -ErrorAction SilentlyContinue |
            Select-Object -First 1
        if ($found) { $appExe = $found.FullName }
    }

    if (-not $appExe) { Start-Sleep -Seconds 2 }
} while (-not $appExe -and (Get-Date) -lt $deadline)

if (-not $appExe) {
    Write-Host "LOCALAPPDATA contents containing Meridian-like names:"
    Get-ChildItem -Path $env:LOCALAPPDATA -Force -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -match 'Meridian' } |
        Format-Table FullName,Mode,Length -AutoSize
    throw "Timed out locating installed Meridian.exe under $env:LOCALAPPDATA after 30 seconds"
}

$installDir = Split-Path -Parent $appExe
Write-Host "Installed application: $appExe"

Write-Step "Launching installed Meridian"
Start-Process -FilePath $appExe | Out-Null

Write-Step "Waiting for bundled daemon staging"
Wait-Until { (Test-Path $daemonExe) -and (Test-Path $backendMarker) } $StartupTimeoutSeconds "staged daemon and backend marker"

Write-Step "Verifying daemon launch registration"
& schtasks /Query /TN "Meridian Daemon" *> $null
$daemonTaskExists = ($LASTEXITCODE -eq 0)
$fallbackExists = Test-Path $daemonFallback
Assert-True ($daemonTaskExists -or $fallbackExists) "Neither the Meridian Daemon task nor Startup fallback exists"

Write-Step "Waiting for daemon process"
Wait-Until { $null -ne (Get-Process -Name 'meridian' -ErrorAction SilentlyContinue) } $StartupTimeoutSeconds "Meridian daemon process"

Write-Step "Running health probe"
$healthText = & $daemonExe doctor --json 2>&1
$doctorExit = $LASTEXITCODE
$health = $null
try {
    $health = ($healthText | Out-String) | ConvertFrom-Json
} catch {
    $health = $null
}

if ($null -ne $health -and $health.schema_version -eq 1) {
    Write-Host "Using doctor JSON contract"
    Assert-True ($doctorExit -in 0,1) "doctor --json returned unexpected exit code $doctorExit"
    Assert-True ($null -ne $health.counts) "doctor JSON is missing counts"
    Assert-True ($health.checks.Count -gt 0) "doctor JSON contains no checks"

    # A clean CI machine has no tracker/AI configuration, so optional
    # integration warnings are not packaging failures. Assert the
    # installer-owned daemon layer.
    $daemonChecks = @($health.checks | Where-Object { $_.group -eq 'meridian daemon' })
    Assert-True ($daemonChecks.Count -gt 0) "doctor JSON contains no meridian daemon checks"
    $daemonCritical = @($daemonChecks | Where-Object { $_.status -eq 'critical' })
    Assert-True ($daemonCritical.Count -eq 0) ("daemon health contains critical checks: " + ($daemonCritical | ConvertTo-Json -Compress))
} else {
    # Stable releases published before the JSON contract still expose the
    # five-column porcelain format. This path lets the fast workflow validate
    # the actual Windows install lifecycle using the latest shipped installer.
    Write-Host "doctor --json unavailable; falling back to porcelain"
    $porcelain = & $daemonExe doctor --porcelain 2>&1
    $porcelainExit = $LASTEXITCODE
    Assert-True ($porcelainExit -in 0,1) "doctor --porcelain returned unexpected exit code $porcelainExit"

    $rows = @($porcelain | ForEach-Object {
        $cols = $_ -split "`t", 5
        if ($cols.Count -eq 5) {
            [pscustomobject]@{
                status = $cols[0]
                group = $cols[1]
                name = $cols[2]
                detail = $cols[3]
                remedy = $cols[4]
            }
        }
    })
    $daemonRows = @($rows | Where-Object { $_.group -eq 'meridian daemon' })
    Assert-True ($daemonRows.Count -gt 0) "doctor porcelain contains no meridian daemon checks"
    $daemonFailures = @($daemonRows | Where-Object { $_.status -eq 'fail' })
    Assert-True ($daemonFailures.Count -eq 0) ("daemon health contains failing checks: " + ($daemonFailures | ConvertTo-Json -Compress))
}

Write-Step "Verifying Meridian data directory"
Assert-True (Test-Path $meridianDir) "~/.meridian was not created"

Write-Step "Stopping the installed tray before cleanup"
Get-Process -ErrorAction SilentlyContinue |
    Where-Object { $_.Path -eq $appExe } |
    Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 2

Write-Step "Running Meridian cleanup command"
$uninstallOutput = & $daemonExe uninstall --purge --yes 2>&1
$uninstallExit = $LASTEXITCODE
Write-Host ($uninstallOutput | Out-String)
Assert-True ($uninstallExit -eq 0) "meridian uninstall exited with code $uninstallExit"

Write-Step "Verifying daemon registration cleanup"
& schtasks /Query /TN "Meridian Daemon" *> $null
Assert-True ($LASTEXITCODE -ne 0) "Meridian Daemon scheduled task still exists after uninstall"
Assert-True (-not (Test-Path $daemonFallback)) "MeridianDaemon.vbs still exists after uninstall"
Assert-True (-not (Test-Path $trayFallback)) "MeridianTray.vbs still exists after uninstall"

Write-Step "Running NSIS uninstaller"
$uninstaller = Get-ChildItem -Path $installDir -Filter '*uninstall*.exe' -File -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $uninstaller) {
    $candidate = Join-Path $installDir 'uninstall.exe'
    if (Test-Path $candidate) { $uninstaller = Get-Item $candidate }
}
Assert-True ($null -ne $uninstaller) "Could not find NSIS uninstaller under $installDir"
$nsisUninstall = Start-Process -FilePath $uninstaller.FullName -ArgumentList '/S' -Wait -PassThru
Assert-True ($nsisUninstall.ExitCode -eq 0) "NSIS uninstaller exited with code $($nsisUninstall.ExitCode)"

Write-Step "Verifying application cleanup"
Wait-Until { -not (Test-Path $appExe) } 30 "Meridian.exe removal"
Get-Process -Name 'Meridian','meridian','meridian-tray' -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue

Write-Host ""
Write-Host "[ok] Windows installed-product smoke test passed"
