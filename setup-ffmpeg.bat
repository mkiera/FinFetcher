@echo off
cd /d "%~dp0"

echo ========================================
echo   FinFetcher - FFmpeg Setup Helper
echo ========================================
echo.

REM Check if ffmpeg folder already exists
if exist "ffmpeg\ffmpeg.exe" (
    echo [OK] FFmpeg is already set up!
    echo.
    ffmpeg\ffmpeg.exe -version 2>nul | findstr "ffmpeg version"
    echo.
    pause
    exit /b 0
)

echo FFmpeg is required to build the executable.
echo.
echo Please follow these steps:
echo.
echo 1. Download FFmpeg essentials build from:
echo    https://www.gyan.dev/ffmpeg/builds/
echo.
echo    Look for: ffmpeg-release-essentials.zip
echo.
echo 2. Extract the ZIP file
echo.
echo 3. Copy these files from the extracted "bin" folder:
echo      - ffmpeg.exe
echo      - ffprobe.exe
echo.
echo 4. Create a folder called "ffmpeg" in this directory
echo    and paste the files there.
echo.
echo Expected structure:
echo    %~dp0ffmpeg\
echo        ffmpeg.exe
echo        ffprobe.exe
echo.
echo ========================================

REM Create the ffmpeg folder for the user
if not exist "ffmpeg" (
    echo.
    echo Creating ffmpeg folder for you...
    mkdir ffmpeg
    echo Done! Now paste ffmpeg.exe and ffprobe.exe into the ffmpeg folder.
)

echo.
echo After setup, run "build exe.bat" to create the executable.
echo.
pause
