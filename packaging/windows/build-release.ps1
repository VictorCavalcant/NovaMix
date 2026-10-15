<#
.SYNOPSIS
    Builds a NovaMix release: binaries, portable zip, checksums and (optionally) the installer.

.DESCRIPTION
    1. Runs the test suite (skip with -SkipTests).
    2. Builds novamix.exe and novamix-cli.exe in release mode.
    3. Signs the binaries when NOVAMIX_SIGN_CERT (certificate thumbprint) is set.
    4. Copies everything to dist\NovaMix and creates a portable zip.
    5. Compiles packaging\windows\novamix.iss when Inno Setup (ISCC.exe) is available.
    6. Writes SHA256SUMS.txt for integrity verification.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File packaging\windows\build-release.ps1
#>
param(
    [switch]$SkipTests,
    [switch]$IncludeTestSignedDriver,
    [string]$TimestampUrl = "http://timestamp.digicert.com"
)

$ErrorActionPreference = "Stop"
$root = Resolve-Path (Join-Path $PSScriptRoot "..\..")
Set-Location $root

$version = (Select-String -Path "Cargo.toml" -Pattern '^version = "(.+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
Write-Host "NovaMix $version"

if (-not $SkipTests) {
    cargo test --workspace --release
    if ($LASTEXITCODE -ne 0) { throw "tests failed" }
}

cargo build --release -p novamix -p novamix-cli
if ($LASTEXITCODE -ne 0) { throw "build failed" }

$dist = Join-Path $root "dist\NovaMix"
if (Test-Path $dist) { Remove-Item -Recurse -Force $dist }
New-Item -ItemType Directory -Force (Join-Path $dist "docs") | Out-Null

Copy-Item "target\release\novamix.exe", "target\release\novamix-cli.exe" $dist
Copy-Item "README.md" $dist
Copy-Item "docs\*" (Join-Path $dist "docs") -Recurse
$driver = Join-Path $root "platform\windows-driver\package"
if (Test-Path (Join-Path $driver "NovaMixVirtualAudio.inf")) {
    if ((Test-Path (Join-Path $driver "TEST-SIGNED.txt")) -and -not $IncludeTestSignedDriver) {
        Write-Warning "Driver package is TEST-SIGNED (platform\windows-driver\package\TEST-SIGNED.txt): not included. Use -IncludeTestSignedDriver for test VMs only."
    } else {
        Copy-Item $driver (Join-Path $dist "driver") -Recurse
        Write-Host "Including signed driver package"
    }
}

if ($env:NOVAMIX_SIGN_CERT) {
    $signtool = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\signtool.exe" | Sort-Object FullName -Descending | Select-Object -First 1
    if (-not $signtool) { throw "signtool.exe not found (install the Windows SDK)" }
    foreach ($exe in Get-ChildItem $dist -Filter *.exe) {
        & $signtool.FullName sign /sha1 $env:NOVAMIX_SIGN_CERT /fd SHA256 /tr $TimestampUrl /td SHA256 $exe.FullName
        if ($LASTEXITCODE -ne 0) { throw "signing failed: $($exe.Name)" }
    }
} else {
    Write-Warning "NOVAMIX_SIGN_CERT not set: binaries are NOT signed"
}

$zip = Join-Path $root "dist\NovaMix-$version-portable.zip"
if (Test-Path $zip) { Remove-Item $zip }
# Portable build: data stored next to the executable.
Set-Content -Path (Join-Path $dist "portable.txt") -Value "Delete this file to store settings in %APPDATA%\NovaMix."
Compress-Archive -Path "$dist\*" -DestinationPath $zip
Remove-Item (Join-Path $dist "portable.txt")

$iscc = @("${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe", "$env:ProgramFiles\Inno Setup 6\ISCC.exe", "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe") | Where-Object { Test-Path $_ } | Select-Object -First 1
if ($iscc) {
    & $iscc "/DAppVersion=$version" "/DDistDir=$dist" (Join-Path $PSScriptRoot "novamix.iss")
    if ($LASTEXITCODE -ne 0) { throw "installer build failed" }
    if ($env:NOVAMIX_SIGN_CERT) {
        $setup = Join-Path $root "dist\NovaMix-$version-setup.exe"
        & $signtool.FullName sign /sha1 $env:NOVAMIX_SIGN_CERT /fd SHA256 /tr $TimestampUrl /td SHA256 $setup
    }
} else {
    Write-Warning "Inno Setup 6 not found: installer skipped (portable zip created)"
}

$sums = Get-ChildItem (Join-Path $root "dist") -File | Where-Object { $_.Extension -in ".zip", ".exe" } | ForEach-Object {
    "{0}  {1}" -f (Get-FileHash $_.FullName -Algorithm SHA256).Hash.ToLower(), $_.Name
}
$sums | Set-Content (Join-Path $root "dist\SHA256SUMS.txt")
Write-Host "Done. Artifacts in dist\"
