; Installer for Familiar. Build the bundle first (packaging/windows/bundle.sh), then:
;   iscc /DAppVersion=0.1.0 packaging\windows\familiar.iss
; The installer lands in dist\.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif

[Setup]
AppId={{6B1C8C55-7E0B-4D0C-9A53-5E9B4D0F2A71}
AppName=Familiar
AppVersion={#AppVersion}
AppPublisher=Familiar
AppPublisherURL=https://github.com/SpiraMirabilis/familiar
DefaultDirName={autopf}\Familiar
DefaultGroupName=Familiar
DisableProgramGroupPage=yes
; Installs for the current user without asking for administrator rights; "for everyone" is
; offered too.
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
LicenseFile=..\..\LICENSE
SetupIconFile=familiar.ico
UninstallDisplayIcon={app}\bin\familiar.exe
OutputDir=..\..\dist
OutputBaseFilename=familiar-{#AppVersion}-x86_64-windows-setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; Flags: unchecked

[Files]
Source: "..\..\dist\familiar\*"; DestDir: "{app}"; Flags: recursesubdirs ignoreversion

[Icons]
; The app id ties the shortcut to the program's notifications.
Name: "{group}\Familiar"; Filename: "{app}\bin\familiar.exe"; AppUserModelID: "io.github.SpiraMirabilis.Familiar"
Name: "{group}\Familiar (terminal)"; Filename: "{app}\familiar-tui.exe"
Name: "{autodesktop}\Familiar"; Filename: "{app}\bin\familiar.exe"; Tasks: desktopicon; AppUserModelID: "io.github.SpiraMirabilis.Familiar"

[Run]
Filename: "{app}\bin\familiar.exe"; Description: "{cm:LaunchProgram,Familiar}"; Flags: nowait postinstall skipifsilent

[Code]
// GLib starts a small helper (gdbus.exe, from our bin folder) that outlives the app by a few
// seconds. While it runs, Windows won't let its files be removed, so uninstalling right after
// closing Familiar would leave files behind. Stop ours (and only ours) first. Upgrades need
// nothing: Setup closes programs that are using the files it replaces.
procedure StopHelper();
var
  ResultCode: Integer;
begin
  Exec('powershell.exe',
    '-NoProfile -Command "Get-Process gdbus -ErrorAction SilentlyContinue | Where-Object { $_.Path -like ''' + ExpandConstant('{app}') + '\*'' } | Stop-Process -Force"',
    '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usUninstall then
    StopHelper();
end;
