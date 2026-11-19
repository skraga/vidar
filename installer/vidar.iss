; Vidar setup, built with Inno Setup 6 by `cargo xtask installer` from dist\vidar.
;
; - Installs for all users into Program Files: one UAC prompt covers the program and the
;   virtual microphone driver.
; - Upgrades in place (same AppId); the running Vidar is asked to quit first, so the user's
;   own microphone is back before files are replaced.
; - Uninstall quits Vidar, turns off "Start with Windows" and offers to remove the driver
;   and the settings.
; - Update runs from the app pass /SILENT /SUPPRESSMSGBOXES /NORESTART /RELAUNCH.
; - Installs the Microsoft Visual C++ Redistributable (needed by onnxruntime.dll, Voice HD)
;   when it is missing or too old: downloaded from Microsoft, signature checked, run quietly.
;   Without it Vidar still runs; Voice HD falls back to Voice.
; - One script for both architectures: /DArch=x64 or /DArch=arm64.

#ifndef AppVersion
  #error Pass /DAppVersion=x.y.z (cargo xtask installer does)
#endif
#ifndef Arch
  #define Arch "x64"
#endif
#ifndef OutputBase
  #define OutputBase "vidar-" + AppVersion + "-setup"
#endif
; Oldest Visual C++ runtime onnxruntime.dll 1.32 is known to work with (14.40, VS 2022 17.10).
#define VCRedistMinMinor 40
#ifndef SourceDir
  #define SourceDir "..\dist\vidar"
#endif
#ifndef OutputDir
  #define OutputDir "..\dist"
#endif
#ifndef IconFile
  #define IconFile "..\dist\vidar.ico"
#endif

[Setup]
; Never change AppId: it is how later versions find this installation.
AppId={{71C74451-02B1-4EFA-85B4-7E55FF9D23CE}
AppName=Vidar
AppVersion={#AppVersion}
AppVerName=Vidar {#AppVersion}
AppPublisher=Vidar
AppPublisherURL=https://github.com/skraga/vidar
AppSupportURL=https://github.com/skraga/vidar/issues
AppUpdatesURL=https://github.com/skraga/vidar/releases
VersionInfoVersion={#AppVersion}
VersionInfoProductName=Vidar
VersionInfoDescription=Vidar Setup
DefaultDirName={autopf}\Vidar
DisableProgramGroupPage=yes
PrivilegesRequired=admin
#if Arch == "arm64"
ArchitecturesAllowed=arm64
ArchitecturesInstallIn64BitMode=arm64
; Windows 11: VB-CABLE's setup is an x64 program, and Windows 10 on ARM cannot run those.
MinVersion=10.0.22000
#else
; x64compatible includes Windows 11 on ARM, where the x64 build runs emulated.
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; Windows 10 1903, as the README says.
MinVersion=10.0.18362
#endif
WizardStyle=modern
SetupIconFile={#IconFile}
UninstallDisplayIcon={app}\vidar.exe
UninstallDisplayName=Vidar
OutputDir={#OutputDir}
OutputBaseFilename={#OutputBase}
Compression=lzma2/max
SolidCompression=yes
SetupMutex=VidarSetup
; Fallback when Vidar cannot be asked to quit (versions before 0.2, or running under
; another account): Restart Manager closes it, which Vidar also handles by restoring the mic.
CloseApplications=yes
CloseApplicationsFilter=vidar.exe,vidar-cli.exe
RestartApplications=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "driver"; Description: "Install the virtual microphone (VB-CABLE by VB-Audio, donationware)"; Check: not CableInstalled
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#SourceDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{autoprograms}\Vidar"; Filename: "{app}\vidar.exe"
Name: "{autodesktop}\Vidar"; Filename: "{app}\vidar.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\vidar-cli.exe"; Parameters: "endpoints install-driver --icon ""{app}\vidar.exe,0"""; WorkingDir: "{app}"; StatusMsg: "Installing the virtual microphone..."; Tasks: driver; Flags: runhidden waituntilterminated
Filename: "{app}\vidar.exe"; Description: "{cm:LaunchProgram,Vidar}"; Flags: nowait postinstall skipifsilent runasoriginaluser
; An update started from the app: start it again once the files are in place.
Filename: "{app}\vidar.exe"; Check: Relaunch; Flags: nowait runasoriginaluser

[Code]
var
  CableChecked, CableFound, DriverRemoved: Boolean;

{ Is the virtual cable already installed? Asks the new vidar-cli, so it also works on a
  first install. Decides whether the driver task is offered. }
function CableInstalled: Boolean;
var
  Code: Integer;
begin
  if not CableChecked then
  begin
    CableChecked := True;
    ExtractTemporaryFile('vidar-cli.exe');
    CableFound := Exec(ExpandConstant('{tmp}\vidar-cli.exe'), 'endpoints check-cable', '',
      SW_HIDE, ewWaitUntilTerminated, Code) and (Code = 0);
  end;
  Result := CableFound;
end;

{ /RELAUNCH on the command line: the app started this update and wants to come back. }
function Relaunch: Boolean;
var
  I: Integer;
begin
  Result := False;
  for I := 1 to ParamCount do
    if CompareText(ParamStr(I), '/RELAUNCH') = 0 then
      Result := True;
end;

{ Ask a running Vidar to quit and wait until it has (it restores the user's microphone
  first). Exit code 1 = still running; Restart Manager then closes it. }
procedure QuitVidar(Exe: String);
var
  Code: Integer;
begin
  if FileExists(Exe) then
    Exec(Exe, '--quit', '', SW_HIDE, ewWaitUntilTerminated, Code);
end;

{ Is a recent enough Microsoft Visual C++ 2015-2022 Redistributable for this architecture
  installed? It registers itself under VisualStudio\14.0\VC\Runtimes. }
function VCRedistInstalled: Boolean;
var
  Key: String;
  Installed, Major, Minor: Cardinal;
begin
  Key := 'SOFTWARE\Microsoft\VisualStudio\14.0\VC\Runtimes\{#Arch}';
  Result := RegQueryDWordValue(HKLM64, Key, 'Installed', Installed) and (Installed = 1)
    and RegQueryDWordValue(HKLM64, Key, 'Major', Major)
    and RegQueryDWordValue(HKLM64, Key, 'Minor', Minor)
    and ((Major > 14) or ((Major = 14) and (Minor >= {#VCRedistMinMinor})));
end;

{ Signed by Microsoft, and the signature checks out? Asks Windows (Authenticode). }
function SignedByMicrosoft(FileName: String): Boolean;
var
  Code: Integer;
begin
  Result := Exec(ExpandConstant('{sys}\WindowsPowerShell\v1.0\powershell.exe'),
    '-NoProfile -NonInteractive -ExecutionPolicy Bypass -Command "$s = Get-AuthenticodeSignature -LiteralPath ''' +
    FileName + '''; if ($s.Status -eq ''Valid'' -and $s.SignerCertificate.Subject -match ''O=Microsoft Corporation'') { exit 0 } else { exit 1 }"',
    '', SW_HIDE, ewWaitUntilTerminated, Code) and (Code = 0);
end;

{ Download the Redistributable from Microsoft and install it quietly. Returns '' on success,
  otherwise why not. Exit codes: 0 done, 3010 done (restart later), 1638 newer one present. }
function InstallVCRedist: String;
var
  Url, Exe: String;
  Code: Integer;
begin
  Result := '';
  Url := 'https://aka.ms/vs/17/release/vc_redist.{#Arch}.exe';
  try
    DownloadTemporaryFile(Url, 'vc_redist.{#Arch}.exe', '', nil);
  except
    Result := 'it could not be downloaded (' + GetExceptionMessage + ')';
    Exit;
  end;
  Exe := ExpandConstant('{tmp}\vc_redist.{#Arch}.exe');
  if not SignedByMicrosoft(Exe) then
  begin
    Result := 'the download is not signed by Microsoft';
    Exit;
  end;
  if not Exec(Exe, '/install /quiet /norestart', '', SW_HIDE, ewWaitUntilTerminated, Code) then
    Result := 'it could not be started'
  else if (Code <> 0) and (Code <> 3010) and (Code <> 1638) then
    Result := 'its setup ended with code ' + IntToStr(Code);
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  Problem: String;
begin
  { The new vidar.exe sends the request, whichever version is running. }
  ExtractTemporaryFile('vidar.exe');
  QuitVidar(ExpandConstant('{tmp}\vidar.exe'));

  if not VCRedistInstalled then
  begin
    WizardForm.PreparingLabel.Caption := 'Installing the Microsoft Visual C++ Runtime...';
    Problem := InstallVCRedist;
    { Not fatal: Vidar runs without it, only Voice HD needs it. }
    if Problem <> '' then
    begin
      Log('Visual C++ Redistributable not installed: ' + Problem);
      SuppressibleMsgBox('The Microsoft Visual C++ Runtime could not be installed: ' + Problem +
        '.' + #13#10#13#10 + 'Vidar works without it, but Voice HD needs it. You can install ' +
        'it later from https://aka.ms/vs/17/release/vc_redist.{#Arch}.exe', mbInformation,
        MB_OK, IDOK);
    end;
  end;
  Result := '';
end;

function InitializeUninstall: Boolean;
begin
  QuitVidar(ExpandConstant('{app}\vidar.exe'));
  Result := True;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Code: Integer;
  Cli: String;
begin
  if CurUninstallStep = usUninstall then
  begin
    { Before the files go: the driver's own uninstaller is among them. A silent uninstall
      keeps the driver, since other programs may use VB-CABLE too. }
    Cli := ExpandConstant('{app}\vidar-cli.exe');
    if (not UninstallSilent) and FileExists(Cli)
      and Exec(Cli, 'endpoints check-cable', '', SW_HIDE, ewWaitUntilTerminated, Code)
      and (Code = 0) then
    begin
      if MsgBox('Also remove the virtual microphone (VB-CABLE)?' + #13#10#13#10 +
          'Vidar AI Microphone and Vidar Internal disappear from Windows. Keep it if another ' +
          'program uses VB-CABLE.', mbConfirmation, MB_YESNO or MB_DEFBUTTON1) = IDYES then
      begin
        Exec(Cli, 'endpoints uninstall-driver', ExpandConstant('{app}'), SW_HIDE,
          ewWaitUntilTerminated, Code);
        DriverRemoved := Code = 0;
      end;
    end;
    { "Start with Windows" would otherwise keep pointing at the deleted program. }
    RegDeleteValue(HKEY_CURRENT_USER, 'Software\Microsoft\Windows\CurrentVersion\Run', 'Vidar');
  end;

  if CurUninstallStep = usPostUninstall then
  begin
    if (not UninstallSilent) and (MsgBox('Delete your Vidar settings too?', mbConfirmation,
        MB_YESNO or MB_DEFBUTTON2) = IDYES) then
      DelTree(ExpandConstant('{userappdata}\Vidar'), True, True, True);
    { The backup of the cable's original names is only needed while the cable exists. }
    if DriverRemoved then
      DelTree(ExpandConstant('{commonappdata}\Vidar'), True, True, True);
  end;
end;
