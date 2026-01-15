@echo off
cd /d "%~dp0"

echo ========================================
echo   FinFetcher - Starting...
echo ========================================
echo.

REM Check for Python launcher
py -3.13 --version >nul 2>&1
if errorlevel 1 (
    echo [ERROR] Python 3.13 not found.
    echo.
    echo Please install Python 3.13 from https://python.org
    echo Make sure to check "Add Python to PATH" during installation.
    echo.
    pause
    exit /b 1
)

REM Install Python packages
echo [1/4] Installing Python packages...
py -3.13 -m pip install -q flask pywebview yt-dlp
if errorlevel 1 (
    echo [WARNING] pip install may have had issues.
)

REM Verify yt-dlp is accessible
echo [2/4] Verifying yt-dlp...
py -3.13 -m yt_dlp --version >nul 2>&1
if errorlevel 1 (
    echo.
    echo [ERROR] yt-dlp is not working properly!
    echo.
    echo Try these fixes:
    echo   1. Run: py -3.13 -m pip install --upgrade yt-dlp
    echo   2. Or download yt-dlp.exe from:
    echo      https://github.com/yt-dlp/yt-dlp/releases
    echo      and place it in this folder.
    echo.
    pause
    exit /b 1
)

REM Verify pywebview
echo [3/4] Verifying pywebview...
py -3.13 -c "import webview" >nul 2>&1
if errorlevel 1 (
    echo.
    echo [ERROR] pywebview failed to install!
    echo.
    echo Try: py -3.13 -m pip install pywebview
    echo.
    pause
    exit /b 1
)

REM All checks passed, launch app
echo [4/4] All checks passed!
echo.
echo Starting FinFetcher...

REM Find Python 3.13 installation and use pythonw.exe (no console window)
for /f "tokens=*" %%i in ('py -3.13 -c "import sys; print(sys.prefix)"') do set PYTHON_HOME=%%i
start "" "%PYTHON_HOME%\pythonw.exe" "%~dp0main.pyw"

