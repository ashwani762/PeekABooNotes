; Copyright (C) 2026 Ashwani Kumar
; SPDX-License-Identifier: GPL-3.0-or-later
;
; The classic setup wizard for PeekABoo Notes. It installs for the current user only (no
; admin), offers a desktop shortcut and starting with Windows, and its uninstaller asks
; whether to delete the notes too. Build it with packaging\inno\build-setup.ps1, which
; passes the version and the paths.

#ifndef AppVersion
  #define AppVersion "0.3.1"
#endif
#ifndef SourceDir
  #define SourceDir "..\..\target\release"
#endif
#ifndef OutputDir
  #define OutputDir "..\..\target\setup"
#endif

#define AppName "PeekABoo Notes"
#define AppExe "peekaboo-notes.exe"
#define RunKey "Software\Microsoft\Windows\CurrentVersion\Run"

[Setup]
AppId={{FB3540DD-19CC-4530-B084-094A8ED88C05}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName}
AppPublisher=Ashwani Kumar
AppPublisherURL=https://github.com/ashwani762/PeekABooNotes
AppSupportURL=https://github.com/ashwani762/PeekABooNotes/issues
; Per user: %LOCALAPPDATA%\Programs, no admin prompt.
PrivilegesRequired=lowest
DefaultDirName={autopf}\{#AppName}
DisableProgramGroupPage=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.17763
OutputDir={#OutputDir}
OutputBaseFilename=PeekABooNotes-Setup-{#AppVersion}
SetupIconFile=app.ico
UninstallDisplayIcon={app}\app.ico
UninstallDisplayName={#AppName}
WizardStyle=modern
Compression=lzma2/max
SolidCompression=yes
; The app's single-instance mutex: setup and uninstall ask for it to be closed first.
AppMutex=Local\PeekABooNotes.Singleton
ShowLanguageDialog=auto

[Languages]
Name: "en"; MessagesFile: "compiler:Default.isl"
Name: "de"; MessagesFile: "compiler:Languages\German.isl"
Name: "es"; MessagesFile: "compiler:Languages\Spanish.isl"
Name: "fr"; MessagesFile: "compiler:Languages\French.isl"
Name: "ja"; MessagesFile: "compiler:Languages\Japanese.isl"

[CustomMessages]
en.StartWithWindows=Start PeekABoo Notes when Windows starts
de.StartWithWindows=PeekABoo Notes beim Start von Windows öffnen
es.StartWithWindows=Abrir PeekABoo Notes al iniciar Windows
fr.StartWithWindows=Lancer PeekABoo Notes au démarrage de Windows
ja.StartWithWindows=Windows の起動時に PeekABoo Notes を開始する
en.DeleteNotes=Also delete your notes?%n%nThey are in:%n%1%n%nChoose No to keep them: they'll be there if you install PeekABoo Notes again.
de.DeleteNotes=Auch Ihre Notizen löschen?%n%nSie liegen in:%n%1%n%nWählen Sie „Nein“, um sie zu behalten: Nach einer Neuinstallation von PeekABoo Notes sind sie wieder da.
es.DeleteNotes=¿Eliminar también tus notas?%n%nEstán en:%n%1%n%nElige No para conservarlas: seguirán ahí si vuelves a instalar PeekABoo Notes.
fr.DeleteNotes=Supprimer aussi vos notes ?%n%nElles se trouvent dans :%n%1%n%nChoisissez Non pour les garder : vous les retrouverez si vous réinstallez PeekABoo Notes.
ja.DeleteNotes=メモも削除しますか？%n%n保存場所:%n%1%n%n残す場合は［いいえ］を選んでください。PeekABoo Notes を再インストールすると、そのまま使えます。

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "startup"; Description: "{cm:StartWithWindows}"

[Files]
Source: "{#SourceDir}\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
; The MCP bridge assistants start (Settings > Data > AI Assistants).
Source: "{#SourceDir}\peekaboo-mcp.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\lang\*.xml"; DestDir: "{app}\lang"; Flags: ignoreversion
Source: "app.ico"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"; IconFilename: "{app}\app.ico"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; IconFilename: "{app}\app.ico"; Tasks: desktopicon

[Registry]
; The same entry the app's own "Start with Windows" switch writes.
Root: HKCU; Subkey: "{#RunKey}"; ValueType: string; ValueName: "{#AppName}"; ValueData: """{app}\{#AppExe}"""; Tasks: startup

[Run]
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent

[Code]
// Start with Windows may also have been switched on later, in the app's Settings: remove
// the entry either way, but only when it starts this copy (a portable or development copy
// can own the same name).
procedure RemoveStartupEntry();
var
  Value: String;
begin
  if RegQueryStringValue(HKCU, '{#RunKey}', '{#AppName}', Value) and (Pos(Lowercase(ExpandConstant('{app}')), Lowercase(Value)) > 0) then
    RegDeleteValue(HKCU, '{#RunKey}', '{#AppName}');
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Notes: String;
begin
  if CurUninstallStep = usUninstall then
    RemoveStartupEntry();
  if CurUninstallStep = usPostUninstall then
  begin
    // The notes are the one thing worth keeping by default: No is the default answer,
    // and the answer a silent uninstall takes.
    Notes := ExpandConstant('{userdocs}\{#AppName}');
    if DirExists(Notes) and (SuppressibleMsgBox(FmtMessage(CustomMessage('DeleteNotes'), [Notes]), mbConfirmation, MB_YESNO or MB_DEFBUTTON2, IDNO) = IDYES) then
      DelTree(Notes, True, True, True);
  end;
end;
