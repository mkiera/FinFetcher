; FinFetcher installer.
;
; Why this exists: the exe this project used to ship was a PyInstaller --onefile
; build, and Windows Defender and Chrome flagged it. Six builds differing by one
; packaging choice each were scored on VirusTotal, and two controls that did
; nothing but print "hello" scored higher than the real app — so the detections
; were about the packaging, not this project's code. A --onedir build was the only
; variant Microsoft did not flag. Onedir is a folder rather than a single file,
; and asking people to unzip a folder and hunt for the exe inside is worse than
; what they have today, so the folder ships inside this installer instead.
;
; Per-user by design: PrivilegesRequired=lowest means Windows never shows a UAC
; prompt, because an elevation dialog on an unsigned installer is its own scare.
;
; Compile with (both defines optional, see below for the fallbacks):
;   iscc /DVersionNumeric=1.2.4.0 installer.iss
; The output is dist_installer\FinFetcher-Setup.exe.

#define AppName "FinFetcher"
#define AppPublisher "mkiera"
#define AppURL "https://github.com/mkiera/FinFetcher"
#define AppExeName "FinFetcher.exe"

; Where PyInstaller left the --onedir output: FinFetcher.exe plus its _internal
; folder. Relative paths resolve against this script's directory.
#ifndef AppSourceDir
  #define AppSourceDir "dist\FinFetcher"
#endif

; The human-readable version, e.g. 1.2.4f-media-options. Read straight out of
; version.txt — the same file make_version_info.py stamps into the app exe — so
; the number here can never drift from the one the app reports. A build may
; override it with /DAppVersion=...
#ifndef AppVersion
  #if FileExists("version.txt")
    #define VersionFileHandle FileOpen("version.txt")
    #define AppVersion Trim(FileRead(VersionFileHandle))
    #expr FileClose(VersionFileHandle)
  #else
    #define AppVersion "0.0.0"
  #endif
#endif

; A Windows version resource needs four plain numbers, which a tag like
; "1.2.4f-media-options" is not. make_version_info.numeric_version already owns
; the rule for turning one into the other, so the build passes the result in
; rather than this script growing a second parser that could disagree with it.
; 0.0.0.0 is Inno Setup's own default for VersionInfoVersion.
#ifndef VersionNumeric
  #define VersionNumeric "0.0.0.0"
#endif

[Setup]
; This GUID is what identifies the installed product to Windows. It must NEVER
; change: a new AppId makes Windows treat the next release as a different
; program, so it installs alongside the old one instead of replacing it, and the
; stale entry sits in Add/Remove Programs forever.
AppId={{6A29BB89-8456-4003-9223-9B44A0F66834}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher={#AppPublisher}
AppPublisherURL={#AppURL}
AppSupportURL={#AppURL}/issues
AppUpdatesURL={#AppURL}/releases

; Per-user install, no elevation. {localappdata}\Programs is where a
; non-administrative install belongs, and writing there needs no UAC prompt.
PrivilegesRequired=lowest
DefaultDirName={localappdata}\Programs\FinFetcher
DefaultGroupName={#AppName}

; The old build was one exe you double-clicked. Every page that is not asking a
; real question is off, so this stays as close to that as an installer can get:
; what is left is the single "additional tasks" page holding the desktop-shortcut
; checkbox, and its button reads Install.
DisableStartupPrompt=yes
DisableWelcomePage=yes
DisableDirPage=yes
DisableProgramGroupPage=yes
DisableReadyPage=yes
DisableFinishedPage=yes

; Both of these are Inno Setup defaults, spelled out because the updater in
; main.pyw depends on them and deliberately does not pass /DIR or /TASKS. Turning
; either off would move a silently-updated app to the default directory and reset
; the user's desktop-shortcut choice.
UsePreviousAppDir=yes
UsePreviousTasks=yes

; Close a running FinFetcher before overwriting it. The updater exits first and
; passes /CLOSEAPPLICATIONS as well, but a second window — or simply losing the
; race with our own shutdown — would otherwise leave files locked. .pyd joins the
; default filter because a running PyInstaller app holds dozens of them and they
; are DLLs by another extension.
CloseApplications=yes
CloseApplicationsFilter=*.exe,*.dll,*.pyd
; Relaunching the app is the [Run] entry's job and only its job. Letting the
; Restart Manager put it back too is how you end up with two windows fighting
; over the same port. The updater passes /NORESTARTAPPLICATIONS for the same
; reason; this makes it true for interactive installs as well.
RestartApplications=no

Uninstallable=yes
UninstallDisplayName={#AppName}
UninstallDisplayIcon={app}\{#AppExeName}

OutputDir=dist_installer
OutputBaseFilename=FinFetcher-Setup
SetupIconFile=icon.ico
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern

; The setup exe is now the file people download, so it carries a version resource
; of its own for the same reason the app exe does: an executable with no company,
; product or description reads as suspicious to antivirus heuristics.
VersionInfoVersion={#VersionNumeric}
VersionInfoProductVersion={#VersionNumeric}
VersionInfoTextVersion={#AppVersion}
VersionInfoProductName={#AppName}
VersionInfoDescription={#AppName} Setup
VersionInfoCompany={#AppPublisher}
VersionInfoCopyright=MIT License

; No ArchitecturesAllowed or ArchitecturesInstallIn64BitMode on purpose: nothing
; lands under Program Files, so there is no WOW64 redirection to opt out of, and
; the accepted spelling of the 64-bit value changed in Inno Setup 6.3 — pinning
; one would break whichever version the build machine happens to have.

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"

; No [InstallDelete] section on purpose. Clearing the previous build's _internal
; is still necessary — see [Code] below — but [InstallDelete] does it at the wrong
; moment, so the job moved there where it can be undone.

[Files]
; The entire --onedir output: FinFetcher.exe at the top and everything PyInstaller
; collected into _internal below it, which is where sys._MEIPASS points at run
; time and therefore where index.html, script.js, style.css, version.txt and
; icon.ico have to end up. ignoreversion because these files are all ours and a
; .pyd's version resource says nothing about which build it came from.
Source: "{#AppSourceDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
; AppUserModelID matches the ID main.pyw sets via
; SetCurrentProcessExplicitAppUserModelID, so a pinned shortcut and the running
; window are the same taskbar button rather than two.
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExeName}"; AppUserModelID: "FinFetcher.App.1"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExeName}"; AppUserModelID: "FinFetcher.App.1"; Tasks: desktopicon

[Run]
; Deliberately neither "postinstall" nor "skipifsilent". A postinstall entry is a
; checkbox on the Setup Completed page, which is disabled here, and skipifsilent
; would leave a silent update with no app running at all — the updater exits
; before Setup starts and expects Setup to be what brings the app back.
Filename: "{app}\{#AppExeName}"; StatusMsg: "Starting {#AppName}..."; Flags: nowait

[UninstallDelete]
; Uninstalling used to leave the whole of %APPDATA%\FinFetcher behind. It holds
; four things and they do not all deserve the same fate: two of them are not a
; question and go unconditionally, here, while config.json and the downloaded
; ffmpeg are asked about in [Code] below. The third entry is not in %APPDATA% at
; all but belongs with them.
;
; cookies\ is browser cookies yt-dlp exported in Netscape format, which is to say
; live session credentials sitting in a plain text file. Leaving a user's
; logged-in YouTube session on disk after they have removed the program is not
; "preserving their data", it is abandoning credentials, and nobody uninstalls
; hoping to keep it. So it goes, and it is not worth asking about.
;
; updates\ is installer exes the updater downloaded. Pure cache; it is worth
; nothing the moment the app it would have updated is gone.
;
; _internal.old exists only when an upgrade died midway and the [Code] section
; below could not put it back. It is ours, we named it, and removing it is also
; what lets {app} itself disappear. Note that these are directories named in
; full: the wildcard sweep of {app} the Inno Setup docs warn against is a
; different thing, and is still not done here.
Type: filesandordirs; Name: "{userappdata}\FinFetcher\cookies"
Type: filesandordirs; Name: "{userappdata}\FinFetcher\updates"
Type: filesandordirs; Name: "{app}\_internal.old"

[Code]
// Comments in here are // rather than { }, because a brace comment ends at the
// first closing brace and half the things worth naming below are constants like
// {app}. One of those in a comment would silently truncate it.
//
// Upgrades: move the old _internal aside instead of deleting it.
//
// A onedir build is a whole tree, and PyInstaller renames, adds and drops files
// between releases, so an upgrade that merely copies over the top leaves the
// previous build's orphaned DLLs and .pyd files behind for the new app to import
// by accident. Clearing the tree first is therefore right, but the
// [InstallDelete] entry that used to do it was the wrong tool: those entries are
// processed as the first step of installation, before a single new file has been
// copied. Setup does undo a failed or cancelled install — it rolls back through
// the same log the uninstaller uses — but a rollback can only remove what Setup
// added. Nothing brings back what the delete already destroyed. So a copy that
// died halfway, whether from a full disk or from antivirus quarantining a DLL —
// the exact failure this project repackaged itself to dodge — left the user with
// no working app at all, just a shortcut pointing at nothing.
//
// A rename costs nothing and can be reversed, so the old tree is parked under
// another name before the copy, deleted only once the install is past the point
// where it can fail, and moved back if it never got there. The one real cost is
// that both trees exist at once, so an upgrade briefly wants roughly double the
// ~36 MB payload in free space. That is a fair price for never destroying a
// working install to save a few seconds of disk.

var
  // Set while an old _internal is parked under another name, and cleared again
  // once the install can no longer fail. Still set when Setup terminates
  // therefore means the install never completed.
  StaleInternalDir: String;
  // Where the tree belongs, remembered so the restore below never has to expand
  // {app} again at a point where the wizard is already tearing itself down.
  LiveInternalDir: String;

procedure CurStepChanged(CurStep: TSetupStep);
var
  BackupDir: String;
begin
  if CurStep = ssInstall then begin
    // ssInstall runs before any file is copied and after CloseApplications has
    // already shut the running app down, so nothing of ours should still hold
    // the tree open.
    LiveInternalDir := ExpandConstant('{app}\_internal');
    BackupDir := ExpandConstant('{app}\_internal.old');
    // A backup already sitting here is from an earlier attempt that died before
    // it could tidy up, and it is 36 MB of it. Whatever it holds is older than
    // anything this run will produce, so it goes either way — including when the
    // live tree is missing entirely, which is the case where the earlier attempt
    // could not even put it back and this run is the repair.
    if DirExists(BackupDir) then
      DelTree(BackupDir, True, True, True);
    if DirExists(LiveInternalDir) then begin
      if RenameFile(LiveInternalDir, BackupDir) then
        StaleInternalDir := BackupDir
      else
        // Locked anyway, or {app} somehow straddles a volume boundary. Fall
        // through and let the file copy overwrite in place: that can leave
        // orphans, which is no worse than what this installer did before it
        // existed, and is much better than refusing to install at all.
        Log('Could not move the previous _internal aside; installing over it.');
    end;
  end else if CurStep = ssPostInstall then begin
    // Setup can no longer fail or be cancelled once it reaches here, so the old
    // tree is finally safe to drop. Clearing the variable is also the signal to
    // DeinitializeSetup that there is nothing left to undo.
    if StaleInternalDir <> '' then begin
      DelTree(StaleInternalDir, True, True, True);
      StaleInternalDir := '';
    end;
  end;
end;

procedure DeinitializeSetup();
begin
  // Arriving here with the variable still set means ssPostInstall never ran, so
  // the install failed or was cancelled. Setup's own rollback finished long
  // before this point and took the half-copied files with it, so moving the old
  // tree back leaves the user on the version they started with rather than on
  // nothing at all.
  if StaleInternalDir <> '' then begin
    Log('Install did not complete; restoring the previous _internal.');
    DelTree(LiveInternalDir, True, True, True);
    if not RenameFile(StaleInternalDir, LiveInternalDir) then
      Log('Could not restore _internal; it is still on disk as _internal.old.');
  end;
end;

// Uninstall: what is kept is a question, not a decision.
//
// Silently deleting someone's settings and a 30 MB ffmpeg they may well want
// again is presumptuous, and leaving both on disk forever is litter. Neither is
// obviously right, so this asks — which is consistent with the rest of the file,
// where the only wizard page left alive is the one asking a real question. The
// answer defaults to keeping, because a hurried click should not be able to
// destroy anything. cookies\ and updates\ are not part of the question; the
// [UninstallDelete] section above says why those go regardless.

var
  RemoveUserData: Boolean;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  DataDir: String;
begin
  if CurUninstallStep = usUninstall then begin
    // Asked here and not in InitializeUninstall, which runs before Windows' own
    // "are you sure" prompt — asking what to keep before the user has confirmed
    // they want the thing gone is backwards. usUninstall is the last hook before
    // removal actually starts.
    //
    // Two guards, because they cover different flags: UninstallSilent is /SILENT
    // or /VERYSILENT, neither of which suppresses message boxes on its own, and
    // SuppressibleMsgBox covers /SUPPRESSMSGBOXES. Both land on keep, matching
    // the default button, because an unattended uninstall has nobody present to
    // consent to losing a user's settings.
    if UninstallSilent then
      RemoveUserData := False
    else
      RemoveUserData := SuppressibleMsgBox(
        'Also remove FinFetcher''s settings and its downloaded copy of ffmpeg?'
        + #13#10#13#10 +
        'Choose No to keep them, ready for a future reinstall.',
        mbConfirmation, MB_YESNO or MB_DEFBUTTON2, IDNO) = IDYES;
  end else if CurUninstallStep = usPostUninstall then begin
    // usPostUninstall runs after the uninstall-delete entries above, so cookies\
    // and updates\ are already gone by the time this reads the folder.
    DataDir := ExpandConstant('{userappdata}\FinFetcher');
    if RemoveUserData then begin
      DeleteFile(DataDir + '\config.json');
      // Only ever the copy the app downloaded for itself. Anyone who pointed
      // config.json's ffmpeg_path at their own build has that folder somewhere
      // else entirely, and it is not ours to delete.
      DelTree(DataDir + '\ffmpeg', True, True, True);
    end;
    // RemoveDir removes an empty directory only, which is exactly the guard
    // wanted here: whatever the user chose to keep, or dropped in there by hand,
    // keeps the folder alive instead of being swept up with it.
    RemoveDir(DataDir);
  end;
end;
