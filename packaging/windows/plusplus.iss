; Build with ISCC.exe /DAppVersion=<version> packaging\windows\plusplus.iss.
; Install for the current user so setup does not need administrator privileges.

[Setup]
AppId=plusplus
AppName=plusplus
AppVersion={#AppVersion}
DefaultDirName={userpf}\plusplus
DefaultGroupName=plusplus
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\..\target\dist
OutputBaseFilename=plusplus-{#AppVersion}-x86_64-windows-setup
SetupIconFile=..\..\crates\app\assets\icon\icon.ico
UninstallDisplayIcon={app}\plusplus.exe
Compression=lzma2
SolidCompression=yes
WizardStyle=modern

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; Flags: unchecked

[Files]
Source: "..\..\target\release\plusplus.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\README.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\plusplus"; Filename: "{app}\plusplus.exe"
Name: "{autodesktop}\plusplus"; Filename: "{app}\plusplus.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\plusplus.exe"; Description: "Launch plusplus"; Flags: nowait postinstall skipifsilent
