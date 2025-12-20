@echo off
cd /d "%~dp0"

REM Try to use py launcher (preferred on Windows with multiple Python versions)
py --version >nul 2>&1
if errorlevel 1 (
    echo [ERROR] Python launcher 'py' not found.
    echo Please install Python from https://python.org
    pause
    exit /b 1
)

REM Install requirements using py launcher
echo Checking dependencies...
py -m pip install -q flask pywebview yt-dlp

REM Test if webview works
py -c "import webview" >nul 2>&1
if errorlevel 1 (
    echo.
    echo [ERROR] pywebview failed to install.
    echo Try: py -m pip install pywebview
    pause
    exit /b 1
)

REM Launch app without console using pyw
echo Starting Aura Downloader...
start "" pyw "%~dp0main.pyw"
