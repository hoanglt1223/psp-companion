; Inno Setup 6 script for the Windows host installer.
; Build (paths are relative to this file): iscc /DAppVersion=0.2.0 companion-host.iss
; Defaults: SourceDir = target\release, OutputDir = out\packages at the repository root.
; Installs companion-host.exe, adds a firewall rule for the private network, and optionally starts it at sign-in.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef SourceDir
  #define SourceDir "..\..\..\target\release"
#endif
#ifndef OutputDir
  #define OutputDir "..\..\..\out\packages"
#endif
#define Root "..\..\.."

[Setup]
AppId={{6F1C2B7E-3A54-4F0B-9C1D-5E2A8B7C4D10}
AppName=PSP Companion
AppVersion={#AppVersion}
AppPublisher=PSP Companion
AppPublisherURL=https://github.com/hoanglt1223/psp-companion
DefaultDirName={autopf}\PSP Companion
DefaultGroupName=PSP Companion
DisableProgramGroupPage=yes
; The firewall rule needs administrator rights.
PrivilegesRequired=admin
; Autostart is written to HKCU on purpose: the elevated installer still runs as the signed-in user.
UsedUserAreasWarning=no
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir={#OutputDir}
OutputBaseFilename=companion-host-v{#AppVersion}-windows-x64-setup
LicenseFile={#Root}\LICENSE
UninstallDisplayIcon={app}\companion-host.exe
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes

[Tasks]
Name: "autostart"; Description: "Start PSP Companion when I sign in"
Name: "firewall"; Description: "Allow PSP connections through Windows Firewall (private networks)"

[Files]
Source: "{#SourceDir}\companion-host.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Root}\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"
Source: "{#Root}\THIRD_PARTY.md"; DestDir: "{app}"
Source: "{#Root}\docs\user\*.md"; DestDir: "{app}\docs"

[Icons]
Name: "{group}\PSP Companion"; Filename: "{app}\companion-host.exe"
Name: "{group}\PSP Companion settings"; Filename: "http://127.0.0.1:47863/settings"
Name: "{group}\Uninstall PSP Companion"; Filename: "{uninstallexe}"

[Registry]
; Per-user autostart. The installer runs elevated but as the signed-in user, so HKCU is that user's hive.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "PSP Companion"; \
  ValueData: """{app}\companion-host.exe"""; Tasks: autostart; Flags: uninsdeletevalue

[Run]
Filename: "{sys}\netsh.exe"; \
  Parameters: "advfirewall firewall delete rule name=""PSP Companion"""; \
  Flags: runhidden; Tasks: firewall
Filename: "{sys}\netsh.exe"; \
  Parameters: "advfirewall firewall add rule name=""PSP Companion"" dir=in action=allow profile=private program=""{app}\companion-host.exe"" enable=yes"; \
  Flags: runhidden; Tasks: firewall
Filename: "{app}\companion-host.exe"; Description: "Start PSP Companion now"; \
  Flags: nowait postinstall skipifsilent runasoriginaluser

[UninstallRun]
Filename: "{sys}\taskkill.exe"; Parameters: "/IM companion-host.exe /F"; Flags: runhidden; RunOnceId: "StopHost"
Filename: "{sys}\netsh.exe"; Parameters: "advfirewall firewall delete rule name=""PSP Companion"""; \
  Flags: runhidden; RunOnceId: "DelFirewall"
