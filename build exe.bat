@echo off
cd /d "%~dp0"

echo ========================================
echo   FinFetcher - Build EXE
echo ========================================
echo.

REM Read version from version.txt
set /p VERSION=<version.txt

REM Check for PyInstaller
py -3.13 -m PyInstaller --version >nul 2>&1
if errorlevel 1 (
    echo [INFO] Installing PyInstaller...
    py -3.13 -m pip install pyinstaller
)

REM Set output path
set OUTPUT_DIR=%USERPROFILE%\Downloads
set EXE_NAME=FinFetcher

echo.
echo [1/3] Building EXE (this may take a few minutes)...
echo       FFmpeg will be auto-downloaded on first run.
echo.

REM Version resource, generated from version.txt (antivirus scores an exe
REM without one as suspicious)
py -3.13 make_version_info.py version_info.txt

REM Build with PyInstaller - no ffmpeg bundled (auto-downloads on first run).
REM --noupx matters: packed binaries pick up antivirus detections on their own.
py -3.13 -m PyInstaller --onefile --windowed --noupx --name %EXE_NAME% --distpath %OUTPUT_DIR% ^
    --icon "icon.ico" ^
    --version-file "version_info.txt" ^
    --add-data "index.html;." ^
    --add-data "style.css;." ^
    --add-data "script.js;." ^
    --add-data "version.txt;." ^
    --add-data "icon.ico;." ^
    --clean --noconfirm main.pyw

if errorlevel 1 (
    echo.
    echo [ERROR] Build failed!
    pause
    exit /b 1
)

echo.
echo [2/3] Cleaning up temp files...
rmdir /s /q build 2>nul
del *.spec 2>nul

echo.
echo [3/3] Build complete!
echo.
echo ========================================
echo   Output: %OUTPUT_DIR%\%EXE_NAME%.exe
echo   FFmpeg: Auto-downloads on first run
echo ========================================
echo.
pause
