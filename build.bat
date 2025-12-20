@echo off
echo ========================================
echo   Aura Downloader - Build Script
echo ========================================
echo.

REM Check if PyInstaller is installed
pip show pyinstaller >nul 2>&1
if errorlevel 1 (
    echo [ERROR] PyInstaller is not installed.
    echo Run: pip install pyinstaller
    pause
    exit /b 1
)

echo [1/2] Building executable...
pyinstaller --onefile --windowed --name "AuraDownloader" --add-data "index.html;." --add-data "style.css;." --add-data "script.js;." main.pyw

echo.
echo [2/2] Cleaning up...
rmdir /s /q build 2>nul
del AuraDownloader.spec 2>nul

echo.
echo ========================================
echo   Build complete!
echo   Output: dist\AuraDownloader.exe
echo ========================================
pause
