@echo off
cd /d "%~dp0"

echo ========================================
echo   Aura Downloader - Build ZIP
echo ========================================
echo.

REM Read version from version.txt
set /p VERSION=<version.txt

REM Set output path with version
set OUTPUT=%USERPROFILE%\Downloads\AuraDownloader_v%VERSION%.zip

REM Remove old zip if exists
if exist "%OUTPUT%" del "%OUTPUT%"

REM Create temp folder with only needed files
echo Building v%VERSION%...
set TEMP_DIR=%TEMP%\AuraDownloader_build
if exist "%TEMP_DIR%" rmdir /s /q "%TEMP_DIR%"
mkdir "%TEMP_DIR%"

REM Copy only the needed files
copy "index.html" "%TEMP_DIR%\" >nul
copy "style.css" "%TEMP_DIR%\" >nul
copy "script.js" "%TEMP_DIR%\" >nul
copy "main.pyw" "%TEMP_DIR%\" >nul
copy "run.bat" "%TEMP_DIR%\" >nul
copy "requirements.txt" "%TEMP_DIR%\" >nul
copy "version.txt" "%TEMP_DIR%\" >nul

REM Create ZIP using PowerShell
echo Creating ZIP...
powershell -Command "Compress-Archive -Path '%TEMP_DIR%\*' -DestinationPath '%OUTPUT%' -Force"

REM Cleanup
rmdir /s /q "%TEMP_DIR%"

echo.
echo ========================================
echo   Build complete!
echo   Output: %OUTPUT%
echo ========================================
echo.
pause
