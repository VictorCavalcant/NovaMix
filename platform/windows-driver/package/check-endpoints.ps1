<#
.SYNOPSIS
    Checks that Windows and NovaMix see the three NovaMix endpoints.

.DESCRIPTION
    1. Lists the NovaMix audio endpoints known to Windows (Get-PnpDevice -Class AudioEndpoint).
    2. Runs "novamix-cli devices" and checks for "NovaMix driver installed: true".
    3. With -SoakSeconds N, also runs "novamix-cli run --seconds N" (use 1800 for the
       30-minute underrun test) and shows its output.

    Does not need administrator rights.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File .\check-endpoints.ps1 -Cli C:\NovaMix\novamix-cli.exe -SoakSeconds 1800
#>
param(
    [string]$Cli,
    [int]$SoakSeconds = 0
)

$ErrorActionPreference = "Stop"
$Endpoints = @("NovaMix Virtual Playback", "NovaMix Mic Feed", "NovaMix Virtual Mic")
$ok = $true

$found = @(Get-PnpDevice -Class AudioEndpoint -PresentOnly -ErrorAction SilentlyContinue | Where-Object { $_.FriendlyName -like "NovaMix*" })
$found | Sort-Object FriendlyName | Format-Table Status, FriendlyName -AutoSize | Out-String | Write-Host
foreach ($name in $Endpoints) {
    if (-not ($found | Where-Object { $_.FriendlyName -like "$name*" })) {
        Write-Warning "Windows endpoint missing: $name"
        $ok = $false
    }
}

if (-not $Cli) {
    $candidates = @(
        (Join-Path $PSScriptRoot "..\..\..\target\release\novamix-cli.exe"),
        (Join-Path $PSScriptRoot "..\..\..\target\debug\novamix-cli.exe"),
        (Join-Path $PSScriptRoot "novamix-cli.exe")
    )
    $Cli = $candidates | Where-Object { Test-Path $_ } | Select-Object -First 1
    if (-not $Cli) { $Cli = (Get-Command novamix-cli.exe -ErrorAction SilentlyContinue).Source }
}

if ($Cli) {
    $output = & $Cli devices 2>&1 | Out-String
    Write-Host $output
    if ($output -notmatch "NovaMix driver installed: true") {
        Write-Warning "novamix-cli does not report the NovaMix driver as installed."
        $ok = $false
    }
    if ($SoakSeconds -gt 0) {
        Write-Host "Running novamix-cli run --seconds $SoakSeconds ..."
        & $Cli run --seconds $SoakSeconds 2>&1 | Out-String | Write-Host
        if ($LASTEXITCODE -ne 0) { $ok = $false }
    }
} else {
    Write-Warning "novamix-cli.exe not found: pass -Cli <path> to check the app side."
}

if ($ok) { Write-Host "OK"; exit 0 } else { exit 1 }
