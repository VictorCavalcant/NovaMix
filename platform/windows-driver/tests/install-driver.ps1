<#
.SYNOPSIS
    Installs the NovaMix virtual audio driver on a TEST machine (VM with test signing).

.DESCRIPTION
    Run from an elevated PowerShell on the test VM, inside the driver package folder
    (NovaMixVirtualAudio.inf/.sys/.cat + scripts), or from platform\windows-driver\tests
    (then ..\package is used).

    Prerequisite for test-signed packages (once per VM, Secure Boot disabled):
        bcdedit /set testsigning on
        (reboot)

    Uses the same script as the product installer (novamix-driver.ps1 -Action Install):
    saves the default devices, trusts the test certificate (TEST-SIGNED packages only),
    creates ROOT\NovaMixVirtualAudio, runs pnputil /add-driver /install, waits for the
    three endpoints and rolls back on failure. Log: %ProgramData%\NovaMix\logs\driver-install.log

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File .\install-driver.ps1
#>
param(
    [string]$PackageDir,
    [int]$TimeoutSeconds = 60
)

$ErrorActionPreference = "Stop"

if (-not $PackageDir) {
    $PackageDir = if (Test-Path (Join-Path $PSScriptRoot "NovaMixVirtualAudio.inf")) { $PSScriptRoot } else { Join-Path $PSScriptRoot "..\package" }
}
$PackageDir = (Resolve-Path $PackageDir).Path
$driverScript = @((Join-Path $PackageDir "novamix-driver.ps1"), (Join-Path $PSScriptRoot "..\scripts\novamix-driver.ps1")) |
    Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $driverScript) { throw "novamix-driver.ps1 not found (build the package with scripts\build-driver.ps1)" }

if (Test-Path (Join-Path $PackageDir "TEST-SIGNED.txt")) {
    $bcd = (bcdedit /enum "{current}") -join "`n"
    if ($bcd -notmatch "testsigning\s+(Yes|Sim)") {
        Write-Warning "Test signing seems to be OFF. Run 'bcdedit /set testsigning on' (Secure Boot disabled) and reboot, or the driver will not load."
    }
}

& $driverScript -Action Install -PackageDir $PackageDir -TimeoutSeconds $TimeoutSeconds
$code = $LASTEXITCODE

Get-PnpDevice -Class AudioEndpoint -PresentOnly -ErrorAction SilentlyContinue |
    Where-Object { $_.FriendlyName -like "NovaMix*" } |
    Sort-Object FriendlyName | Format-Table Status, FriendlyName, InstanceId -AutoSize | Out-String | Write-Host

if ($code -ne 0) { Write-Error "Install failed: see $env:ProgramData\NovaMix\logs\driver-install.log"; exit 1 }
Write-Host "NovaMix driver installed: the three endpoints are present."
exit 0
