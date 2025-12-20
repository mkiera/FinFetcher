@echo off
cd /d "%~dp0"

echo ========================================
echo   FinFetcher - Build EXE
echo ========================================
echo.

REM Read version from version.txt
set /p VERSION=<version.txt

REM Check for PyInstaller
py -m PyInstaller --version >nul 2>&1
if errorlevel 1 (
    echo [INFO] Installing PyInstaller...
    py -m pip install pyinstaller
)

REM Set output path
set OUTPUT_DIR=%USERPROFILE%\Downloads
set EXE_NAME=FinFetcher_v%VERSION%

echo.
echo [1/3] Building EXE (this may take a few minutes)...
echo.

REM Build with PyInstaller
py -m PyInstaller --onefile --windowed --name %EXE_NAME% --distpath %OUTPUT_DIR% --add-data "index.html;." --add-data "style.css;." --add-data "script.js;." --add-data "version.txt;." --clean --noconfirm main.pyw

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
echo ========================================
echo.
pause
