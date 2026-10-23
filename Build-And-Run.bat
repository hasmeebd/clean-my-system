@echo off
cd /d "%~dp0"
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0build.ps1"
if exist "%~dp0dist\Clean-My-System.exe" start "" "%~dp0dist\Clean-My-System.exe"
pause
