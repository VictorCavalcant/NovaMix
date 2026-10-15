<#
.SYNOPSIS
    Builds the NovaMix virtual audio driver and writes the signed package to
    platform\windows-driver\package.

.DESCRIPTION
    1. Builds NovaMixVirtualAudio.vcxproj (x64) with MSBuild from Visual Studio 2022
       + WDK 10.0.26100. The WDK stamps the INF, runs InfVerif and creates the catalog.
    2. Copies NovaMixVirtualAudio.sys/.inf/.cat to package\.
    3. Signs the .sys and the .cat:
       * with the CurrentUser\My certificate whose SHA-1 thumbprint is in
         NOVAMIX_DRIVER_SIGN_CERT, or
       * with a self-signed "NovaMix Test Driver Signing" certificate (created on first
         use). Its public part goes to package\NovaMixTestCert.cer and the package is
         marked TEST-SIGNED.txt: it only installs on machines with test signing on.
       Packages for end users must be attestation-signed by the Microsoft Hardware Dev
       Center (docs/decisions.md, D6).
    4. Copies novamix-driver.ps1 (used by the product installer) and the test-VM
       install/uninstall/check scripts into the package.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File platform\windows-driver\scripts\build-driver.ps1
#>
param(
    [ValidateSet("Release", "Debug")]
    [string]$Configuration = "Release",
    [string]$TimestampUrl = ""
)

$ErrorActionPreference = "Stop"

$driverDir = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$project = Join-Path $driverDir "NovaMixVirtualAudio.vcxproj"
$packageDir = Join-Path $driverDir "package"
$testCertSubject = "CN=NovaMix Test Driver Signing"

# --- Tools ---------------------------------------------------------------------------------
$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
if (-not (Test-Path $vswhere)) { throw "Visual Studio 2022 not found (vswhere.exe missing)." }
$vs = & $vswhere -latest -version "[17.0,18.0)" -requires Component.Microsoft.Windows.DriverKit -property installationPath
if (-not $vs) { throw "Visual Studio 2022 with the 'Windows Driver Kit' component not found." }
$msbuild = Join-Path $vs "MSBuild\Current\Bin\amd64\MSBuild.exe"

$kit = "${env:ProgramFiles(x86)}\Windows Kits\10"
$signtool = Join-Path $kit "bin\10.0.26100.0\x64\signtool.exe"
if (-not (Test-Path $signtool)) { throw "signtool.exe from the Windows SDK/WDK 10.0.26100 not found." }
if (-not (Test-Path (Join-Path $kit "Include\10.0.26100.0\km\portcls.h"))) { throw "WDK 10.0.26100 not installed." }

# --- Build ---------------------------------------------------------------------------------
& $msbuild $project /t:Rebuild "/p:Configuration=$Configuration" /p:Platform=x64 /nologo /v:minimal
if ($LASTEXITCODE -ne 0) { throw "driver build failed" }

$buildPackage = Join-Path $driverDir "build\x64\$Configuration\NovaMixVirtualAudio"

if (Test-Path $packageDir) { Remove-Item -Recurse -Force $packageDir }
New-Item -ItemType Directory $packageDir | Out-Null
Copy-Item (Join-Path $buildPackage "NovaMixVirtualAudio.sys") $packageDir
Copy-Item (Join-Path $buildPackage "NovaMixVirtualAudio.inf") $packageDir
# The WDK writes the catalog name in lower case; the INF names it NovaMixVirtualAudio.cat.
Copy-Item (Join-Path $buildPackage "novamixvirtualaudio.cat") (Join-Path $packageDir "NovaMixVirtualAudio.cat")

# --- Sign ----------------------------------------------------------------------------------
$thumbprint = $env:NOVAMIX_DRIVER_SIGN_CERT
$testSigned = -not $thumbprint
if ($testSigned) {
    $cert = Get-ChildItem Cert:\CurrentUser\My |
        Where-Object { $_.Subject -eq $testCertSubject -and $_.NotAfter -gt (Get-Date).AddDays(30) -and $_.HasPrivateKey } |
        Sort-Object NotAfter -Descending | Select-Object -First 1
    if (-not $cert) {
        $cert = New-SelfSignedCertificate -Type CodeSigningCert -Subject $testCertSubject `
            -CertStoreLocation Cert:\CurrentUser\My -KeyAlgorithm RSA -KeyLength 3072 `
            -HashAlgorithm SHA256 -NotAfter (Get-Date).AddYears(3)
        Write-Host "Created test certificate $($cert.Thumbprint) in CurrentUser\My."
    }
    $thumbprint = $cert.Thumbprint
    Export-Certificate -Cert $cert -FilePath (Join-Path $packageDir "NovaMixTestCert.cer") | Out-Null
}

# Authenticode hashes skip the signature, so signing the .sys after the catalog was
# generated keeps the catalog valid.
foreach ($file in "NovaMixVirtualAudio.sys", "NovaMixVirtualAudio.cat") {
    $signArgs = @("sign", "/s", "My", "/sha1", $thumbprint, "/fd", "SHA256")
    if ($TimestampUrl) { $signArgs += @("/tr", $TimestampUrl, "/td", "SHA256") }
    & $signtool @signArgs (Join-Path $packageDir $file)
    if ($LASTEXITCODE -ne 0) { throw "signing failed: $file" }
}

# --- Scripts -------------------------------------------------------------------------------
# novamix-driver.ps1 is what the product installer runs; the others are for test VMs.
Copy-Item (Join-Path $driverDir "scripts\novamix-driver.ps1") $packageDir
foreach ($script in "install-driver.ps1", "uninstall-driver.ps1", "check-endpoints.ps1") {
    Copy-Item (Join-Path $driverDir "tests\$script") $packageDir
}

if ($testSigned) {
    Set-Content -Path (Join-Path $packageDir "TEST-SIGNED.txt") -Encoding ASCII -Value @(
        "This NovaMix driver package is signed with a self-signed TEST certificate.",
        "It only installs on machines with test signing enabled (bcdedit /set testsigning on).",
        "Use it on a test VM: run install-driver.ps1 as administrator.",
        "Do not ship it: end-user packages must be attestation-signed (docs/decisions.md, D6)."
    )
    Write-Warning "Package is TEST-SIGNED: for VMs with test signing only."
}

Get-ChildItem $packageDir | Format-Table Name, Length -AutoSize | Out-String | Write-Host
Write-Host "Driver package ready: $packageDir"
