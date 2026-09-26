#ifndef AppName
  #define AppName "FinFetcher"
#endif
#ifndef AppExeName
  #define AppExeName "FinFetcher.exe"
#endif
#ifndef AppId
  #define AppId "{{6A29BB89-8456-4003-9223-9B44A0F66834}"
#endif
#ifndef StorageId
  #define StorageId "FinFetcher"
#endif
#ifndef AppDataDir
  #define AppDataDir "{userappdata}\" + StorageId
#endif
#ifndef AppSourceDir
  #define AppSourceDir "dist\FinFetcher"
#endif
#ifndef AppVersion
  #define AppVersion Trim(FileRead(FileOpen("version.txt")))
#endif
#ifndef VersionNumeric
  #define VersionNumeric "0.0.0.0"
#endif
#ifndef OutputName
  #define OutputName "FinFetcher-Setup"
#endif
#ifndef AppRunParameters
  #define AppRunParameters ""
#endif
#ifndef WebView2Bootstrapper
  #define WebView2Bootstrapper "build\webview2\MicrosoftEdgeWebView2Setup.exe"
#endif

[Setup]
AppId={#AppId}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher=mkiera
AppPublisherURL=https://github.com/mkiera/FinFetcher
AppSupportURL=https://github.com/mkiera/FinFetcher/issues
AppUpdatesURL=https://github.com/mkiera/FinFetcher/releases
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
DefaultDirName={localappdata}\Programs\{#StorageId}
DefaultGroupName={#AppName}
DisableStartupPrompt=yes
DisableWelcomePage=yes
DisableDirPage=yes
DisableProgramGroupPage=yes
DisableReadyPage=yes
DisableFinishedPage=yes
UsePreviousAppDir=yes
UsePreviousTasks=yes
CloseApplications=yes
CloseApplicationsFilter=*.exe,*.dll,*.pyd
RestartApplications=no
Uninstallable=yes
UninstallDisplayName={#AppName}
UninstallDisplayIcon={app}\{#AppExeName}
OutputDir=dist_installer
OutputBaseFilename={#OutputName}
SetupIconFile=icon.ico
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
VersionInfoVersion={#VersionNumeric}
VersionInfoProductVersion={#VersionNumeric}
VersionInfoProductTextVersion={#AppVersion}
VersionInfoTextVersion={#AppVersion}
VersionInfoProductName={#AppName}
VersionInfoDescription={#AppName} Setup
VersionInfoCompany=mkiera
VersionInfoCopyright=GPL-3.0-or-later

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"

[Files]
#ifndef SkipWebView2
Source: "{#WebView2Bootstrapper}"; DestName: "MicrosoftEdgeWebView2Setup.exe"; Flags: dontcopy
#endif
Source: "{#AppSourceDir}\{#AppExeName}"; DestDir: "{app}"; Flags: ignoreversion
#ifdef TestCopyFailure
Source: "{#TestCopyFailure}"; DestDir: "{app}"; Flags: external ignoreversion
#endif
Source: "{#AppSourceDir}\*"; DestDir: "{app}"; Excludes: "{#AppExeName}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
#ifndef TestNoIcons
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExeName}"; AppUserModelID: "FinFetcher.App.1"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExeName}"; AppUserModelID: "FinFetcher.App.1"; Tasks: desktopicon
#endif

[Run]
#ifdef TestStateDir
Filename: "{app}\{#AppExeName}"; Parameters: "--hidden --state-dir ""{#TestStateDir}"""; StatusMsg: "Starting {#AppName}..."; Flags: nowait runasoriginaluser
#else
Filename: "{app}\{#AppExeName}"; Parameters: "{#AppRunParameters}"; StatusMsg: "Starting {#AppName}..."; Flags: nowait runasoriginaluser
#endif

[UninstallDelete]
Type: filesandordirs; Name: "{#AppDataDir}\cookies"
Type: filesandordirs; Name: "{#AppDataDir}\updates"

[Code]
var
  LiveExe: String;
  SavedExe: String;
  LiveInternal: String;
  HadInternal: Boolean;
  RemoveUserData: Boolean;

function HasWebView2(): Boolean;
var
  Version: String;
  Key: String;
begin
#ifdef TestRuntimeMissing
  Result := False;
#else
  Key := 'SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}';
  Result := (RegQueryStringValue(HKLM32, Key, 'pv', Version) and (Version <> '') and (Version <> '0.0.0.0'))
    or (RegQueryStringValue(HKCU, Key, 'pv', Version) and (Version <> '') and (Version <> '0.0.0.0'));
#endif
end;

function PrepareWebView2(): String;
var
  ExitCode: Integer;
begin
  Result := '';
#ifndef SkipWebView2
  if not HasWebView2() then begin
    try
      ExtractTemporaryFile('MicrosoftEdgeWebView2Setup.exe');
      if not Exec(ExpandConstant('{tmp}\MicrosoftEdgeWebView2Setup.exe'), '/silent /install',
          ExpandConstant('{tmp}'), SW_HIDE, ewWaitUntilTerminated, ExitCode) then
        Result := 'Microsoft Edge WebView2 could not be started. Install the WebView2 Runtime and try again.'
      else if (ExitCode <> 0) or not HasWebView2() then
        Result := 'Microsoft Edge WebView2 could not be installed (code ' + IntToStr(ExitCode)
          + '). Check your internet connection, install the WebView2 Runtime, and try again. Your existing FinFetcher installation has not been changed.';
    except
      Result := 'The WebView2 Runtime installer could not be prepared. Download FinFetcher-Setup.exe again and retry.';
    end;
  end;
#endif
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  PreviousExe: String;
  PreviousInternal: String;
begin
  Result := PrepareWebView2();
  if Result <> '' then
    Exit;
  LiveExe := ExpandConstant('{app}\{#AppExeName}');
  LiveInternal := ExpandConstant('{app}\_internal');
  PreviousExe := LiveExe + '.previous';
  PreviousInternal := LiveInternal + '.old';
  if FileExists(PreviousExe) then begin
    if not FileExists(LiveExe) then begin
      if not RenameFile(PreviousExe, LiveExe) then
        Result := 'The previous application could not be restored. Close FinFetcher and try again.';
    end else
      Result := 'An earlier update left an application backup. Keep a copy of ' + PreviousExe + ' and remove it before retrying.';
  end;
  if (Result = '') and DirExists(PreviousInternal) then begin
    if not DirExists(LiveInternal) then begin
      if not RenameFile(PreviousInternal, LiveInternal) then
        Result := 'The previous Python runtime could not be restored. Close FinFetcher and try again.';
    end else
      Result := 'An earlier update left _internal.old beside the application. Keep a copy of that folder and remove it before retrying.';
  end;
end;

procedure BackupApplication();
begin
  HadInternal := DirExists(LiveInternal);
  if FileExists(LiveExe) then begin
    if not FileCopy(LiveExe, LiveExe + '.previous', True) then
      RaiseException('The previous application could not be saved. Close FinFetcher and try again.');
    SavedExe := LiveExe + '.previous';
  end;
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssInstall then begin
    if FileExists(LiveExe) then begin
      // Older updaters wait for the installer log before exiting.
      Sleep(3000);
    end;
    BackupApplication();
  end else if CurStep = ssPostInstall then begin
    if SavedExe <> '' then begin
      DeleteFile(SavedExe);
      SavedExe := '';
    end;
    if HadInternal then
      DelTree(LiveInternal, True, True, True);
  end;
end;

procedure DeinitializeSetup();
begin
  if SavedExe <> '' then begin
    if FileExists(LiveExe) and (GetSHA256OfFile(LiveExe) = GetSHA256OfFile(SavedExe)) then
      DeleteFile(SavedExe)
    else begin
      DeleteFile(LiveExe);
      if not RenameFile(SavedExe, LiveExe) then
        Log('The previous executable remains at ' + SavedExe);
    end;
  end;
end;

function IsTemporaryCookieDirectory(Name: String): Boolean;
var
  I: Integer;
begin
  Result := False;
  if (Length(Name) <> 14) or (Copy(Name, 1, 8) <> 'cookies-') then
    Exit;
  for I := 9 to 14 do
    if not (((Name[I] >= 'a') and (Name[I] <= 'z'))
      or ((Name[I] >= 'A') and (Name[I] <= 'Z'))
      or ((Name[I] >= '0') and (Name[I] <= '9'))) then
      Exit;
  Result := True;
end;

procedure RemoveManagedData(DataDir: String);
var
  Found: TFindRec;
begin
  DeleteFile(DataDir + '\config.json');
  DelTree(DataDir + '\ffmpeg', True, True, True);
  DelTree(DataDir + '\ytdlp', True, True, True);
  DelTree(DataDir + '\ytdlp-bin', True, True, True);
  DelTree(DataDir + '\deno', True, True, True);
  DelTree(DataDir + '\tools', True, True, True);
  DelTree(DataDir + '\cache', True, True, True);
  DelTree(DataDir + '\webview2', True, True, True);
  DelTree(DataDir + '\updates', True, True, True);
  if FindFirst(DataDir + '\cookies-*', Found) then begin
    try
      repeat
        if IsTemporaryCookieDirectory(Found.Name) and ((Found.Attributes and FILE_ATTRIBUTE_DIRECTORY) <> 0) then
          DelTree(DataDir + '\' + Found.Name, True, True, True);
      until not FindNext(Found);
    finally
      FindClose(Found);
    end;
  end;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  DataDir: String;
begin
  if CurUninstallStep = usUninstall then begin
#ifdef TestRemoveUserData
    RemoveUserData := True;
#else
    if UninstallSilent then
      RemoveUserData := False
    else
      RemoveUserData := SuppressibleMsgBox(
        'Also remove FinFetcher''s settings and downloaded media tools?'
        + #13#10#13#10 +
        'Choose No to keep them for a future reinstall.',
        mbConfirmation, MB_YESNO or MB_DEFBUTTON2, IDNO) = IDYES;
#endif
  end else if CurUninstallStep = usPostUninstall then begin
    DataDir := ExpandConstant('{#AppDataDir}');
    if RemoveUserData then
      RemoveManagedData(DataDir);
    RemoveDir(DataDir);
#ifdef TestUninstallMarker
  end else if CurUninstallStep = usDone then begin
    SaveStringToFile('{#TestUninstallMarker}', 'done', False);
#endif
  end;
end;
