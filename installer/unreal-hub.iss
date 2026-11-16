#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif

[Setup]
AppId={{8F6C2A3E-5B71-4C0E-9D2A-6E4B1F3C7A90}
AppName=Unreal Hub
AppVersion={#AppVersion}
AppVerName=Unreal Hub {#AppVersion}
AppPublisher=Mossworm
AppPublisherURL=https://github.com/Mossworm/unreal-hub
AppSupportURL=https://github.com/Mossworm/unreal-hub/issues
AppUpdatesURL=https://github.com/Mossworm/unreal-hub/releases
DefaultDirName={localappdata}\Programs\Unreal Hub
DefaultGroupName=Unreal Hub
DisableProgramGroupPage=yes
DisableDirPage=auto
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\dist
OutputBaseFilename=UnrealHub-{#AppVersion}-setup
SetupIconFile=..\assets\app.ico
UninstallDisplayIcon={app}\unreal-hub.exe
UninstallDisplayName=Unreal Hub
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes
RestartApplications=no
ShowLanguageDialog=auto

[Languages]
Name: "korean"; MessagesFile: "compiler:Languages\Korean.isl"
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "..\target\release\unreal-hub.exe"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Unreal Hub"; Filename: "{app}\unreal-hub.exe"
Name: "{autodesktop}\Unreal Hub"; Filename: "{app}\unreal-hub.exe"; Tasks: desktopicon

[Registry]
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: none; ValueName: "UnrealHub"; Flags: uninsdeletevalue dontcreatekey

[Run]
Filename: "{app}\unreal-hub.exe"; Description: "{cm:LaunchProgram,Unreal Hub}"; Flags: nowait postinstall skipifsilent
Filename: "{app}\unreal-hub.exe"; Flags: nowait; Check: WizardSilent
