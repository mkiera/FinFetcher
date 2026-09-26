@echo off
cd /d "%~dp0"
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\build.ps1
if errorlevel 1 exit /b 1
pause
