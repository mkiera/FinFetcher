@echo off
cd /d "%~dp0"

echo ========================================
echo   FinFetcher - Build EXE + installer
echo ========================================
echo.

REM Read version from version.txt
set /p VERSION=<version.txt

REM Check for PyInstaller. Installing the whole pinned build set rather than
REM just PyInstaller, so a local build uses the same versions CI does — that is
REM the point of pinning them.
py -3.13 -m PyInstaller --version >nul 2>&1
if errorlevel 1 (
    echo [INFO] Installing the pinned build requirements...
    py -3.13 -m pip install -r requirements-build.txt
)

set EXE_NAME=FinFetcher
set OUTPUT_DIR=%USERPROFILE%\Downloads

echo.
echo [1/4] Building the app folder (this may take a few minutes)...
echo       FFmpeg will be auto-downloaded on first run.
echo.

REM Version resource, generated from version.txt (antivirus scores an exe
REM without one as suspicious)
py -3.13 make_version_info.py version_info.txt

REM Records which commit this build came from, so the app can tell itself apart
REM from the other builds of the same version listed in the Alpha tab
py -3.13 make_build_info.py build_info.json

REM Build with PyInstaller - no ffmpeg bundled (auto-downloads on first run).
REM --onedir, not --onefile: the single-file build is what Defender and Chrome
REM flag, and a onedir build was the only variant Microsoft left alone.
REM --noupx matters for the same reason: packed binaries pick up antivirus
REM detections on their own. Output goes to dist\FinFetcher\, which installer.iss
REM then packages whole.
py -3.13 -m PyInstaller --onedir --windowed --noupx --name %EXE_NAME% ^
    --icon "icon.ico" ^
    --version-file "version_info.txt" ^
    --add-data "index.html;." ^
    --add-data "style.css;." ^
    --add-data "script.js;." ^
    --add-data "version.txt;." ^
    --add-data "build_info.json;." ^
    --add-data "icon.ico;." ^
    --add-data "fonts;fonts" ^
    --clean --noconfirm main.pyw

if errorlevel 1 (
    echo.
    echo [ERROR] Build failed!
    pause
    exit /b 1
)

echo.
echo [2/4] Looking for Inno Setup...

REM Inno Setup does not add itself to PATH, and winget installs it per-user into
REM %LOCALAPPDATA%\Programs rather than Program Files, so all three are searched.
set "ISCC="
for /f "delims=" %%i in ('where iscc.exe 2^>nul') do set "ISCC=%%i"
if not defined ISCC for /d %%d in ("%ProgramFiles(x86)%\Inno Setup *") do if exist "%%d\ISCC.exe" set "ISCC=%%d\ISCC.exe"
if not defined ISCC for /d %%d in ("%ProgramFiles%\Inno Setup *") do if exist "%%d\ISCC.exe" set "ISCC=%%d\ISCC.exe"
if not defined ISCC for /d %%d in ("%LOCALAPPDATA%\Programs\Inno Setup *") do if exist "%%d\ISCC.exe" set "ISCC=%%d\ISCC.exe"

if not defined ISCC (
    echo.
    echo ========================================
    echo   No installer was built
    echo ========================================
    echo.
    echo   Inno Setup is not installed on this machine. It is what turns
    echo   dist\%EXE_NAME%\ into the single FinFetcher-Setup.exe we ship.
    echo.
    echo   Install it, then run this script again:
    echo       winget install -e --id JRSoftware.InnoSetup
    echo   or download it from https://jrsoftware.org/isdl.php
    echo.
    echo   The app itself is built and runnable right now at:
    echo       %CD%\dist\%EXE_NAME%\%EXE_NAME%.exe
    echo.
    pause
    exit /b 0
)

echo       Found: %ISCC%
echo.
echo [3/4] Building the installer...
echo.

REM A Windows version resource needs four plain numbers, and version.txt can say
REM something like 1.2.4f-media-options. make_version_info.py already owns that
REM rule, so reuse it instead of teaching installer.iss a second one. Written via
REM a temp file rather than a for /f, which cmd mangles on the parentheses in the
REM Python one-liner.
set VERNUM=0.0.0.0
py -3.13 -c "import make_version_info as m; open('build_vernum.tmp','w').write('.'.join(str(n) for n in m.numeric_version(open('version.txt', encoding='utf-8').read())))"
if exist build_vernum.tmp set /p VERNUM=<build_vernum.tmp
del build_vernum.tmp 2>nul

"%ISCC%" "/O%OUTPUT_DIR%" "/DVersionNumeric=%VERNUM%" "installer.iss"

if errorlevel 1 (
    echo.
    echo [ERROR] Installer build failed!
    pause
    exit /b 1
)

echo.
echo [4/4] Cleaning up temp files...
rmdir /s /q build 2>nul
del *.spec 2>nul

echo.
echo ========================================
echo   Output: %OUTPUT_DIR%\FinFetcher-Setup.exe
echo   Version: %VERSION% (%VERNUM%)
echo   Installs per-user, no admin prompt
echo   FFmpeg: Auto-downloads on first run
echo ========================================
echo.
pause
