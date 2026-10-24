@echo off
cd /d "%~dp0"
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0build.ps1"
if exist "%~dp0dist\System-Care-Suite.exe" start "" "%~dp0dist\System-Care-Suite.exe"
pause
