@echo off
setlocal
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0run-portable.ps1" %*
endlocal
