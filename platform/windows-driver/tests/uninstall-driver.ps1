<#
.SYNOPSIS
    Removes the NovaMix virtual audio driver from a TEST machine (VM).

.DESCRIPTION
    Run from an elevated PowerShell. Uses the same script as the product uninstaller
    (novamix-driver.ps1 -Action Uninstall): removes every ROOT\NovaMixVirtualAudio device and
    every NovaMixVirtualAudio.inf package (pnputil /remove-device, /delete-driver; the oemNN.inf
    names come from Get-WindowsDriver, so any display language works) and restores the default
    devices that pointed to NovaMix endpoints.
    With -RemoveCertificate also removes the NovaMix test certificate from LocalMachine Root
    and TrustedPublisher.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File .\uninstall-driver.ps1 -RemoveCertificate
#>
param(
    [switch]$RemoveCertificate
)

$ErrorActionPreference = "Stop"
$TestCertSubject = "CN=NovaMix Test Driver Signing"

$driverScript = @((Join-Path $PSScriptRoot "novamix-driver.ps1"), (Join-Path $PSScriptRoot "..\scripts\novamix-driver.ps1")) |
    Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $driverScript) { throw "novamix-driver.ps1 not found" }

& $driverScript -Action Uninstall
$code = $LASTEXITCODE

if ($RemoveCertificate) {
    foreach ($store in "Root", "TrustedPublisher") {
        Get-ChildItem "Cert:\LocalMachine\$store" | Where-Object { $_.Subject -eq $TestCertSubject } | Remove-Item
    }
    Write-Host "Test certificate removed."
}

if ($code -ne 0) { Write-Error "Uninstall incomplete: see $env:ProgramData\NovaMix\logs\driver-uninstall.log"; exit 1 }
Write-Host "NovaMix driver removed."
exit 0
