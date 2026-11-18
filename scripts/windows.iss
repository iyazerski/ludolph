#ifndef BuildProfile
  #define BuildProfile "release"
#endif
#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif

[Setup]
AppId=dev.iyazerski.ludolph
AppName=Ludolph
AppVersion={#AppVersion}
VersionInfoVersion={#AppVersion}
AppPublisher=Ludolph
DefaultDirName={localappdata}\Programs\Ludolph
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
SourceDir=..
OutputDir=target
OutputBaseFilename=Ludolph-Setup
SetupIconFile=assets\Ludolph.ico
UninstallDisplayIcon={app}\Ludolph.exe
CloseApplications=yes
RestartApplications=no

[Files]
Source: "target\{#BuildProfile}\ludolph.exe"; DestDir: "{app}"; DestName: "Ludolph.exe"
Source: "LICENSE"; DestDir: "{app}"
Source: "THIRD_PARTY_LICENSES.md"; DestDir: "{app}"
Source: "target\wsl\ludolph-engine-linux-*.tar.gz"; DestDir: "{app}\wsl"

[Icons]
Name: "{autoprograms}\Ludolph"; Filename: "{app}\Ludolph.exe"

[Run]
Filename: "{app}\Ludolph.exe"; Description: "Open Ludolph"; Flags: nowait postinstall skipifsilent
Filename: "{app}\Ludolph.exe"; Flags: nowait skipifnotsilent; Check: IsUpdate

[Code]
function IsUpdate: Boolean;
begin
  Result := ExpandConstant('{param:UPDATE|0}') = '1';
end;
