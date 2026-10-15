; NovaMix installer (Inno Setup 6).
; Build with packaging\windows\build-release.ps1, which passes /DAppVersion and /DDistDir.
;
; * Per-user install by default (no administrator rights needed for the app).
; * The virtual audio driver is an optional component that requires elevation
;   and is only offered when the signed driver package is present in DistDir\driver.
;   It is installed/removed by {app}\driver\novamix-driver.ps1, run through UAC: the script
;   creates the root-enumerated ROOT\NovaMixVirtualAudio device (pnputil alone cannot),
;   runs pnputil, checks the three endpoints, rolls back on failure and saves/restores the
;   default playback/recording devices (%ProgramData%\NovaMix).
; * User data (%APPDATA%\NovaMix) is kept on uninstall unless the user opts in to remove it.

#ifndef AppVersion
  #define AppVersion "0.1.0"
#endif
#ifndef DistDir
  #define DistDir "..\..\dist\NovaMix"
#endif
#if FileExists(DistDir + "\driver\NovaMixVirtualAudio.inf") && FileExists(DistDir + "\driver\novamix-driver.ps1")
  #define HasDriver
#endif

[Setup]
AppId={{6B8D1E5A-3F0C-4C55-9E1B-5A2F0E3D7C41}
AppName=NovaMix
AppVersion={#AppVersion}
AppPublisher=NovaMix contributors
DefaultDirName={autopf}\NovaMix
DefaultGroupName=NovaMix
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
OutputBaseFilename=NovaMix-{#AppVersion}-setup
OutputDir={#DistDir}\..
Compression=lzma2/ultra
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
UninstallDisplayIcon={app}\novamix.exe
MinVersion=10.0.19045

[Languages]
Name: "ptbr"; MessagesFile: "compiler:Languages\BrazilianPortuguese.isl"
Name: "en"; MessagesFile: "compiler:Default.isl"

[CustomMessages]
ptbr.Autostart=Iniciar o NovaMix com o Windows (na bandeja)
en.Autostart=Start NovaMix with Windows (in the tray)
ptbr.DriverComponent=Driver de áudio virtual NovaMix (requer administrador)
en.DriverComponent=NovaMix virtual audio driver (requires administrator)
ptbr.DriverWhyAdmin=Agora o Windows vai pedir permissão de administrador.%n%nEla é usada só para instalar o driver de áudio virtual NovaMix (dispositivos "NovaMix Virtual Playback", "NovaMix Mic Feed" e "NovaMix Virtual Mic"). O NovaMix em si não precisa de administrador.%n%nSeus dispositivos de som padrão são salvos antes e restaurados se o Windows trocá-los.
en.DriverWhyAdmin=Windows will now ask for administrator permission.%n%nIt is only used to install the NovaMix virtual audio driver (devices "NovaMix Virtual Playback", "NovaMix Mic Feed" and "NovaMix Virtual Mic"). NovaMix itself does not need administrator rights.%n%nYour default sound devices are saved first and restored if Windows switches them.
ptbr.DriverInstalling=Instalando o driver de áudio virtual...
en.DriverInstalling=Installing the virtual audio driver...
ptbr.DriverFailed=O driver de áudio virtual não foi instalado (código %1). Nada foi deixado para trás e seus dispositivos padrão foram mantidos.%n%nO NovaMix funciona sem ele usando cabos virtuais de terceiros (VB-CABLE e similares). Detalhes em %2.
en.DriverFailed=The virtual audio driver was not installed (code %1). Nothing was left behind and your default devices were kept.%n%nNovaMix works without it using third-party virtual cables (VB-CABLE and similar). Details in %2.
ptbr.DriverCancelled=A instalação do driver foi cancelada (permissão de administrador negada). O NovaMix foi instalado sem ele.
en.DriverCancelled=Driver installation was cancelled (administrator permission denied). NovaMix was installed without it.
ptbr.RemoveDriver=Remover também o driver de áudio virtual NovaMix?%n%nO Windows vai pedir permissão de administrador. Os dispositivos padrão que apontavam para o NovaMix voltam para os que você usava antes.
en.RemoveDriver=Also remove the NovaMix virtual audio driver?%n%nWindows will ask for administrator permission. Default devices that pointed to NovaMix go back to the ones you used before.
ptbr.RemoveDriverFailed=O driver de áudio virtual não foi removido por completo (código %1). Detalhes em %2. Ele pode ser removido depois pelo Gerenciador de Dispositivos.
en.RemoveDriverFailed=The virtual audio driver was not completely removed (code %1). Details in %2. It can be removed later from Device Manager.
ptbr.RemoveData=Remover também as configurações e perfis do NovaMix?
en.RemoveData=Also remove NovaMix settings and profiles?

[Types]
Name: "full"; Description: "Full"
Name: "app"; Description: "App only"

[Components]
Name: "app"; Description: "NovaMix"; Types: full app; Flags: fixed
#ifdef HasDriver
Name: "driver"; Description: "{cm:DriverComponent}"; Types: full
#endif

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "autostart"; Description: "{cm:Autostart}"; Flags: unchecked

[Files]
Source: "{#DistDir}\novamix.exe"; DestDir: "{app}"; Components: app; Flags: ignoreversion
Source: "{#DistDir}\novamix-cli.exe"; DestDir: "{app}"; Components: app; Flags: ignoreversion
Source: "{#DistDir}\README.md"; DestDir: "{app}"; Components: app; Flags: ignoreversion
Source: "{#DistDir}\docs\*"; DestDir: "{app}\docs"; Components: app; Flags: ignoreversion recursesubdirs
#ifdef HasDriver
Source: "{#DistDir}\driver\*"; DestDir: "{app}\driver"; Components: driver; Flags: ignoreversion recursesubdirs
#endif

[Icons]
Name: "{group}\NovaMix"; Filename: "{app}\novamix.exe"
Name: "{group}\NovaMix (modo seguro)"; Filename: "{app}\novamix.exe"; Parameters: "--safe-mode"
Name: "{autodesktop}\NovaMix"; Filename: "{app}\novamix.exe"; Tasks: desktopicon

[Registry]
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "NovaMix"; ValueData: """{app}\novamix.exe"" --minimized"; Tasks: autostart; Flags: uninsdeletevalue

[Run]
Filename: "{app}\novamix.exe"; Description: "{cm:LaunchProgram,NovaMix}"; Flags: nowait postinstall skipifsilent

[UninstallRun]
Filename: "{sys}\taskkill.exe"; Parameters: "/IM novamix.exe /F"; Flags: runhidden; RunOnceId: "KillNovaMix"

[Code]
const
  NM_ERROR_CANCELLED = 1223;

function DriverScript: String;
begin
  Result := ExpandConstant('{app}\driver\novamix-driver.ps1');
end;

function DriverLog(const Action: String): String;
begin
  Result := ExpandConstant('{commonappdata}\NovaMix\logs\driver-') + Action + '.log';
end;

// Runs novamix-driver.ps1 elevated (UAC) and waits. Returns False if it could not be
// started (e.g. UAC denied); otherwise Code is the exit code of the script.
function RunDriverScript(const Action: String; var Code: Integer): Boolean;
var
  Params: String;
begin
  Params := '-NoProfile -ExecutionPolicy Bypass -File "' + DriverScript + '" -Action ' + Action;
  Result := ShellExec('runas', ExpandConstant('{sys}\WindowsPowerShell\v1.0\powershell.exe'),
    Params, ExpandConstant('{app}\driver'), SW_HIDE, ewWaitUntilTerminated, Code);
end;

#ifdef HasDriver
procedure InstallDriver;
var
  Code: Integer;
begin
  SuppressibleMsgBox(CustomMessage('DriverWhyAdmin'), mbInformation, MB_OK, IDOK);
  WizardForm.StatusLabel.Caption := CustomMessage('DriverInstalling');
  if not RunDriverScript('Install', Code) then
  begin
    if Code = NM_ERROR_CANCELLED then
      SuppressibleMsgBox(CustomMessage('DriverCancelled'), mbError, MB_OK, IDOK)
    else
      SuppressibleMsgBox(FmtMessage(CustomMessage('DriverFailed'), [IntToStr(Code), DriverLog('install')]), mbError, MB_OK, IDOK);
  end
  else if Code <> 0 then
    SuppressibleMsgBox(FmtMessage(CustomMessage('DriverFailed'), [IntToStr(Code), DriverLog('install')]), mbError, MB_OK, IDOK);
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if (CurStep = ssPostInstall) and WizardIsComponentSelected('driver') then
    InstallDriver;
end;
#endif

procedure UninstallDriver;
var
  Code: Integer;
begin
  if not FileExists(DriverScript) then
    Exit;
  if SuppressibleMsgBox(CustomMessage('RemoveDriver'), mbConfirmation, MB_YESNO, IDYES) <> IDYES then
    Exit;
  if not RunDriverScript('Uninstall', Code) or (Code <> 0) then
    SuppressibleMsgBox(FmtMessage(CustomMessage('RemoveDriverFailed'), [IntToStr(Code), DriverLog('uninstall')]), mbError, MB_OK, IDOK);
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  DataDir: String;
begin
  // usUninstall runs before the app folder is deleted, while the driver script still exists.
  if CurUninstallStep = usUninstall then
    UninstallDriver;

  if CurUninstallStep = usPostUninstall then
  begin
    DataDir := ExpandConstant('{userappdata}\NovaMix');
    if DirExists(DataDir) then
      if MsgBox(CustomMessage('RemoveData'), mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES then
        DelTree(DataDir, True, True, True);
  end;
end;
