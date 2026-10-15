<#
.SYNOPSIS
    Installs or removes the NovaMix virtual audio driver (used by the NovaMix installer).

.DESCRIPTION
    Must run elevated. Lives next to the driver package (NovaMixVirtualAudio.inf/.sys/.cat);
    the installer copies the package to {app}\driver and runs this script through UAC.

    -Action Install
      1. Saves the current default playback/recording devices (all roles) to
         %ProgramData%\NovaMix\driver-defaults.json.
      2. Trusts NovaMixTestCert.cer, only for TEST-SIGNED packages (test VMs).
      3. Creates the root-enumerated device ROOT\NovaMixVirtualAudio if missing
         (pnputil alone cannot create it).
      4. pnputil /add-driver NovaMixVirtualAudio.inf /install
      5. Waits for the three endpoints. On failure removes the device and the package.
      6. Restores the saved defaults wherever Windows switched them to a NovaMix endpoint.

    -Action Uninstall
      Removes the device(s) and the driver package(s); every default role that pointed to
      a NovaMix endpoint goes back to the saved device (if it still exists).

    -Action ShowDefaults
      Read-only: prints the current default devices and the NovaMix endpoints.

    Exit codes: 0 success, 1 failure (details in %ProgramData%\NovaMix\logs\driver-<action>.log).

.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File novamix-driver.ps1 -Action Install
#>
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet("Install", "Uninstall", "ShowDefaults")]
    [string]$Action,
    [string]$PackageDir = $PSScriptRoot,
    [string]$StateDir = (Join-Path $env:ProgramData "NovaMix"),
    [int]$TimeoutSeconds = 60
)

$ErrorActionPreference = "Stop"

$HardwareId = "ROOT\NovaMixVirtualAudio"
$InfName = "NovaMixVirtualAudio.inf"
$MediaClassGuid = [Guid]"4d36e96c-e325-11ce-bfc1-08002be10318"
$Endpoints = @("NovaMix Virtual Playback", "NovaMix Mic Feed", "NovaMix Virtual Mic")
$DefaultsFile = Join-Path $StateDir "driver-defaults.json"
$Flows = @{ render = 0; capture = 1 }          # EDataFlow
$Roles = @{ console = 0; multimedia = 1; communications = 2 }   # ERole

Add-Type -TypeDefinition @"
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Text;

public static class NovaMixAudioSetup
{
    // ---- Root device creation (what "devcon install" does) --------------------------------
    [StructLayout(LayoutKind.Sequential)]
    struct SP_DEVINFO_DATA
    {
        public int cbSize;
        public Guid ClassGuid;
        public int DevInst;
        public IntPtr Reserved;
    }

    [DllImport("setupapi.dll", SetLastError = true)]
    static extern IntPtr SetupDiCreateDeviceInfoList(ref Guid ClassGuid, IntPtr hwndParent);

    [DllImport("setupapi.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    static extern bool SetupDiCreateDeviceInfoW(IntPtr DeviceInfoSet, string DeviceName, ref Guid ClassGuid,
        string DeviceDescription, IntPtr hwndParent, int CreationFlags, ref SP_DEVINFO_DATA DeviceInfoData);

    [DllImport("setupapi.dll", SetLastError = true)]
    static extern bool SetupDiSetDeviceRegistryPropertyW(IntPtr DeviceInfoSet, ref SP_DEVINFO_DATA DeviceInfoData,
        int Property, byte[] PropertyBuffer, int PropertyBufferSize);

    [DllImport("setupapi.dll", SetLastError = true)]
    static extern bool SetupDiCallClassInstaller(int InstallFunction, IntPtr DeviceInfoSet, ref SP_DEVINFO_DATA DeviceInfoData);

    [DllImport("setupapi.dll", SetLastError = true)]
    static extern bool SetupDiDestroyDeviceInfoList(IntPtr DeviceInfoSet);

    public static void CreateRootDevice(string hardwareId, string className, Guid classGuid)
    {
        IntPtr set = SetupDiCreateDeviceInfoList(ref classGuid, IntPtr.Zero);
        if (set == new IntPtr(-1)) throw new Win32Exception(Marshal.GetLastWin32Error());
        try
        {
            SP_DEVINFO_DATA data = new SP_DEVINFO_DATA();
            data.cbSize = Marshal.SizeOf(typeof(SP_DEVINFO_DATA));
            if (!SetupDiCreateDeviceInfoW(set, className, ref classGuid, null, IntPtr.Zero, 0x1 /* DICD_GENERATE_ID */, ref data))
                throw new Win32Exception(Marshal.GetLastWin32Error());
            byte[] ids = Encoding.Unicode.GetBytes(hardwareId + "\0\0");
            if (!SetupDiSetDeviceRegistryPropertyW(set, ref data, 0x1 /* SPDRP_HARDWAREID */, ids, ids.Length))
                throw new Win32Exception(Marshal.GetLastWin32Error());
            if (!SetupDiCallClassInstaller(0x19 /* DIF_REGISTERDEVICE */, set, ref data))
                throw new Win32Exception(Marshal.GetLastWin32Error());
        }
        finally
        {
            SetupDiDestroyDeviceInfoList(set);
        }
    }

    // ---- Default audio devices -------------------------------------------------------------
    [ComImport, Guid("A95664D2-9614-4F35-A746-DE8DB63617E6"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    interface IMMDeviceEnumerator
    {
        [PreserveSig] int EnumAudioEndpoints(int dataFlow, int stateMask, out IntPtr devices);
        [PreserveSig] int GetDefaultAudioEndpoint(int dataFlow, int role, out IMMDevice endpoint);
        [PreserveSig] int GetDevice([MarshalAs(UnmanagedType.LPWStr)] string id, out IMMDevice device);
    }

    [ComImport, Guid("D666063F-1587-4E43-81F1-B948E807363F"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    interface IMMDevice
    {
        [PreserveSig] int Activate(ref Guid iid, int clsCtx, IntPtr activationParams, out IntPtr iface);
        [PreserveSig] int OpenPropertyStore(int access, out IntPtr properties);
        [PreserveSig] int GetId([MarshalAs(UnmanagedType.LPWStr)] out string id);
        [PreserveSig] int GetState(out int state);
    }

    [ComImport, Guid("BCDE0395-E52F-467C-8E3D-C4579291692E")]
    class MMDeviceEnumerator { }

    // Undocumented but stable since Windows 7 (used by the Sound control panel).
    [ComImport, Guid("F8679F50-850A-41CF-9C72-430F290290C8"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    interface IPolicyConfig
    {
        [PreserveSig] int GetMixFormat(string id, IntPtr format);
        [PreserveSig] int GetDeviceFormat(string id, int defaultFormat, IntPtr format);
        [PreserveSig] int ResetDeviceFormat(string id);
        [PreserveSig] int SetDeviceFormat(string id, IntPtr endpointFormat, IntPtr mixFormat);
        [PreserveSig] int GetProcessingPeriod(string id, int defaultPeriod, IntPtr defaultP, IntPtr minimumP);
        [PreserveSig] int SetProcessingPeriod(string id, IntPtr period);
        [PreserveSig] int GetShareMode(string id, IntPtr mode);
        [PreserveSig] int SetShareMode(string id, IntPtr mode);
        [PreserveSig] int GetPropertyValue(string id, int fxStore, IntPtr key, IntPtr value);
        [PreserveSig] int SetPropertyValue(string id, int fxStore, IntPtr key, IntPtr value);
        [PreserveSig] int SetDefaultEndpoint([MarshalAs(UnmanagedType.LPWStr)] string id, int role);
        [PreserveSig] int SetEndpointVisibility(string id, int visible);
    }

    [ComImport, Guid("870AF99C-171D-4F9E-AF0D-E63DF40C2BC9")]
    class CPolicyConfigClient { }

    // Returns the endpoint id, or null when there is no default device for that flow/role.
    public static string GetDefault(int flow, int role)
    {
        IMMDeviceEnumerator enumerator = (IMMDeviceEnumerator)new MMDeviceEnumerator();
        IMMDevice device;
        if (enumerator.GetDefaultAudioEndpoint(flow, role, out device) != 0 || device == null) return null;
        string id;
        return device.GetId(out id) == 0 ? id : null;
    }

    public static bool IsActive(string id)
    {
        IMMDeviceEnumerator enumerator = (IMMDeviceEnumerator)new MMDeviceEnumerator();
        IMMDevice device;
        if (enumerator.GetDevice(id, out device) != 0 || device == null) return false;
        int state;
        return device.GetState(out state) == 0 && state == 1; // DEVICE_STATE_ACTIVE
    }

    public static void SetDefault(string id, int role)
    {
        IPolicyConfig policy = (IPolicyConfig)new CPolicyConfigClient();
        int hr = policy.SetDefaultEndpoint(id, role);
        if (hr != 0) Marshal.ThrowExceptionForHR(hr);
    }
}
"@

function Get-NovaMixDevices {
    @(Get-PnpDevice -ErrorAction SilentlyContinue | Where-Object { $_.HardwareID -contains $HardwareId })
}

# MMDevice ids of the NovaMix endpoints ("{0.0.0.00000000}.{guid}"), from SWD\MMDEVAPI\<id>.
function Get-NovaMixEndpoints {
    @(Get-PnpDevice -Class AudioEndpoint -ErrorAction SilentlyContinue |
        Where-Object { $_.FriendlyName -like "NovaMix*" } |
        ForEach-Object {
            [pscustomobject]@{
                Name    = $_.FriendlyName
                Status  = $_.Status
                Present = $_.Present
                Id      = ($_.InstanceId -replace "^SWD\\MMDEVAPI\\", "")
            }
        })
}

function Test-NovaMixEndpointId([string]$Id, $NovaMixIds) {
    return $Id -and ($NovaMixIds | Where-Object { $_ -ieq $Id })
}

function Get-Defaults {
    $result = @{}
    foreach ($flow in $Flows.Keys) {
        $result[$flow] = @{}
        foreach ($role in $Roles.Keys) {
            $result[$flow][$role] = [NovaMixAudioSetup]::GetDefault($Flows[$flow], $Roles[$role])
        }
    }
    return $result
}

function Read-SavedDefaults {
    if (-not (Test-Path $DefaultsFile)) { return $null }
    return Get-Content $DefaultsFile -Raw | ConvertFrom-Json
}

# Saves the current defaults, keeping the previous value for roles that already point to a
# NovaMix endpoint (e.g. on a reinstall): those are not what we want to restore later.
function Save-Defaults {
    $novaMixIds = @(Get-NovaMixEndpoints | ForEach-Object { $_.Id })
    $previous = Read-SavedDefaults
    $current = Get-Defaults
    $saved = @{ savedAt = (Get-Date).ToString("o") }
    foreach ($flow in $Flows.Keys) {
        $saved[$flow] = @{}
        foreach ($role in $Roles.Keys) {
            $id = $current[$flow][$role]
            if (Test-NovaMixEndpointId $id $novaMixIds) {
                $id = if ($previous) { $previous.$flow.$role } else { $null }
            }
            $saved[$flow][$role] = $id
        }
    }
    New-Item -ItemType Directory -Force $StateDir | Out-Null
    $saved | ConvertTo-Json -Depth 4 | Set-Content -Path $DefaultsFile -Encoding UTF8
    Write-Host "Saved default devices to $DefaultsFile"
}

# Puts back the saved device for every role whose default is a NovaMix endpoint (or one of
# $NovaMixIds, captured before the endpoints were removed). Roles the user pointed at another
# real device are left alone.
function Restore-Defaults($NovaMixIds, $CurrentDefaults) {
    $saved = Read-SavedDefaults
    if (-not $saved) { Write-Host "No saved default devices."; return }
    foreach ($flow in $Flows.Keys) {
        foreach ($role in $Roles.Keys) {
            $current = $CurrentDefaults[$flow][$role]
            $target = $saved.$flow.$role
            if (-not (Test-NovaMixEndpointId $current $NovaMixIds)) { continue }
            if (-not $target -or -not [NovaMixAudioSetup]::IsActive($target)) {
                Write-Warning "Default $flow/$role is a NovaMix endpoint and the saved device is gone; Windows will choose one."
                continue
            }
            try {
                [NovaMixAudioSetup]::SetDefault($target, $Roles[$role])
                Write-Host "Restored default $flow/$role -> $target"
            } catch {
                Write-Warning "Could not restore default $flow/${role}: $($_.Exception.Message)"
            }
        }
    }
}

function Remove-NovaMixDriver {
    foreach ($device in Get-NovaMixDevices) {
        & pnputil.exe /remove-device $device.InstanceId | Write-Host
        if ($LASTEXITCODE -ne 0 -and $LASTEXITCODE -ne 3010) {
            Write-Warning "pnputil /remove-device $($device.InstanceId) failed ($LASTEXITCODE)"
        }
    }
    $packages = @(Get-WindowsDriver -Online | Where-Object { [IO.Path]::GetFileName($_.OriginalFileName) -ieq $InfName })
    foreach ($package in $packages) {
        & pnputil.exe /delete-driver $package.Driver /uninstall /force | Write-Host
        if ($LASTEXITCODE -ne 0 -and $LASTEXITCODE -ne 3010) {
            Write-Warning "pnputil /delete-driver $($package.Driver) failed ($LASTEXITCODE)"
        }
    }
}

function Wait-Endpoints {
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    do {
        $present = @(Get-NovaMixEndpoints | Where-Object { $_.Present })
        $missing = @($Endpoints | Where-Object { $name = $_; -not ($present | Where-Object { $_.Name -like "$name*" }) })
        if ($missing.Count -eq 0) { return @() }
        Start-Sleep -Seconds 1
    } while ((Get-Date) -lt $deadline)
    return $missing
}

function Install-NovaMixDriver {
    $inf = Join-Path $PackageDir $InfName
    if (-not (Test-Path $inf)) { throw "$InfName not found in $PackageDir" }

    Save-Defaults

    $cer = Join-Path $PackageDir "NovaMixTestCert.cer"
    if ((Test-Path (Join-Path $PackageDir "TEST-SIGNED.txt")) -and (Test-Path $cer)) {
        Write-Warning "TEST-SIGNED package: trusting the test certificate (test machines only)."
        foreach ($store in "Root", "TrustedPublisher") {
            Import-Certificate -FilePath $cer -CertStoreLocation "Cert:\LocalMachine\$store" | Out-Null
        }
    }

    try {
        if ((Get-NovaMixDevices).Count -eq 0) {
            [NovaMixAudioSetup]::CreateRootDevice($HardwareId, "MEDIA", $MediaClassGuid)
            Write-Host "Created device $HardwareId"
        }

        & pnputil.exe /add-driver $inf /install | Write-Host
        $code = $LASTEXITCODE
        # 3010 = success, reboot required; 259 = no device needed an update (already installed).
        if ($code -ne 0 -and $code -ne 3010 -and $code -ne 259) { throw "pnputil /add-driver failed ($code)" }

        $missing = Wait-Endpoints
        if ($missing.Count -gt 0) { throw ("endpoints not found after {0}s: {1}" -f $TimeoutSeconds, ($missing -join ", ")) }
    } catch {
        Write-Warning "Install failed: $($_.Exception.Message). Rolling back."
        $novaMixIds = @(Get-NovaMixEndpoints | ForEach-Object { $_.Id })
        $defaults = Get-Defaults
        Remove-NovaMixDriver
        Restore-Defaults $novaMixIds $defaults
        throw
    }

    # Windows may have made a new NovaMix endpoint the default device.
    Restore-Defaults @(Get-NovaMixEndpoints | ForEach-Object { $_.Id }) (Get-Defaults)
    Write-Host "NovaMix driver installed."
}

function Uninstall-NovaMixDriver {
    # Capture before removal: afterwards the NovaMix endpoints are gone and Windows has
    # already picked other defaults.
    $novaMixIds = @(Get-NovaMixEndpoints | ForEach-Object { $_.Id })
    $defaults = Get-Defaults
    Remove-NovaMixDriver
    Restore-Defaults $novaMixIds $defaults
    if (Test-Path $DefaultsFile) { Remove-Item $DefaultsFile }

    $left = @(Get-NovaMixEndpoints | Where-Object { $_.Present })
    if ($left.Count -gt 0) { throw "NovaMix endpoints still present (a reboot may be needed)." }
    Write-Host "NovaMix driver removed."
}

function Show-Defaults {
    $defaults = Get-Defaults
    foreach ($flow in $Flows.Keys) {
        foreach ($role in $Roles.Keys) { Write-Host ("{0,-8} {1,-15} {2}" -f $flow, $role, $defaults[$flow][$role]) }
    }
    Get-NovaMixEndpoints | Format-Table Name, Status, Present, Id -AutoSize | Out-String | Write-Host
    if (Test-Path $DefaultsFile) { Write-Host "Saved: $DefaultsFile"; Get-Content $DefaultsFile | Write-Host }
}

# ------------------------------------------------------------------------------------------
if ($Action -eq "ShowDefaults") { Show-Defaults; exit 0 }

$principal = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Write-Error "Run elevated (administrator)."
    exit 1
}

$logDir = Join-Path $StateDir "logs"
New-Item -ItemType Directory -Force $logDir | Out-Null
Start-Transcript -Path (Join-Path $logDir ("driver-{0}.log" -f $Action.ToLowerInvariant())) -Force | Out-Null
try {
    if ($Action -eq "Install") { Install-NovaMixDriver } else { Uninstall-NovaMixDriver }
    $exitCode = 0
} catch {
    Write-Host "ERROR: $($_.Exception.Message)"
    $exitCode = 1
} finally {
    Stop-Transcript | Out-Null
}
exit $exitCode
