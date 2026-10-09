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
Name: "urlhandler"; Description: "Open database links (postgres://, mysql://, sqlserver://) with plusplus"

[Files]
Source: "..\..\target\release\plusplus.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\README.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\plusplus"; Filename: "{app}\plusplus.exe"
Name: "{autodesktop}\plusplus"; Filename: "{app}\plusplus.exe"; Tasks: desktopicon

[Registry]
; Database links clicked in a browser launch plusplus with the URL as its argument.
; Keep in sync with dbcore::CONNECTION_URL_SCHEMES.
Root: HKCU; Subkey: "Software\Classes\postgres"; ValueType: string; ValueName: ""; ValueData: "URL:postgres database connection"; Flags: uninsdeletekey; Tasks: urlhandler
Root: HKCU; Subkey: "Software\Classes\postgres"; ValueType: string; ValueName: "URL Protocol"; ValueData: ""; Tasks: urlhandler
Root: HKCU; Subkey: "Software\Classes\postgres\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\plusplus.exe"" ""%1"""; Tasks: urlhandler
Root: HKCU; Subkey: "Software\Classes\postgresql"; ValueType: string; ValueName: ""; ValueData: "URL:postgresql database connection"; Flags: uninsdeletekey; Tasks: urlhandler
Root: HKCU; Subkey: "Software\Classes\postgresql"; ValueType: string; ValueName: "URL Protocol"; ValueData: ""; Tasks: urlhandler
Root: HKCU; Subkey: "Software\Classes\postgresql\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\plusplus.exe"" ""%1"""; Tasks: urlhandler
Root: HKCU; Subkey: "Software\Classes\mysql"; ValueType: string; ValueName: ""; ValueData: "URL:mysql database connection"; Flags: uninsdeletekey; Tasks: urlhandler
Root: HKCU; Subkey: "Software\Classes\mysql"; ValueType: string; ValueName: "URL Protocol"; ValueData: ""; Tasks: urlhandler
Root: HKCU; Subkey: "Software\Classes\mysql\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\plusplus.exe"" ""%1"""; Tasks: urlhandler
Root: HKCU; Subkey: "Software\Classes\mariadb"; ValueType: string; ValueName: ""; ValueData: "URL:mariadb database connection"; Flags: uninsdeletekey; Tasks: urlhandler
Root: HKCU; Subkey: "Software\Classes\mariadb"; ValueType: string; ValueName: "URL Protocol"; ValueData: ""; Tasks: urlhandler
Root: HKCU; Subkey: "Software\Classes\mariadb\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\plusplus.exe"" ""%1"""; Tasks: urlhandler
Root: HKCU; Subkey: "Software\Classes\sqlserver"; ValueType: string; ValueName: ""; ValueData: "URL:sqlserver database connection"; Flags: uninsdeletekey; Tasks: urlhandler
Root: HKCU; Subkey: "Software\Classes\sqlserver"; ValueType: string; ValueName: "URL Protocol"; ValueData: ""; Tasks: urlhandler
Root: HKCU; Subkey: "Software\Classes\sqlserver\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\plusplus.exe"" ""%1"""; Tasks: urlhandler
Root: HKCU; Subkey: "Software\Classes\mssql"; ValueType: string; ValueName: ""; ValueData: "URL:mssql database connection"; Flags: uninsdeletekey; Tasks: urlhandler
Root: HKCU; Subkey: "Software\Classes\mssql"; ValueType: string; ValueName: "URL Protocol"; ValueData: ""; Tasks: urlhandler
Root: HKCU; Subkey: "Software\Classes\mssql\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\plusplus.exe"" ""%1"""; Tasks: urlhandler

[Run]
Filename: "{app}\plusplus.exe"; Description: "Launch plusplus"; Flags: nowait postinstall skipifsilent
