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

[InstallDelete]
; A onedir build is a whole tree, and PyInstaller renames, adds and drops files
; between releases. Without this, every upgrade would leave the previous build's
; orphaned DLLs and .pyd files sitting there for the new app to load by accident.
Type: filesandordirs; Name: "{app}\_internal"

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
