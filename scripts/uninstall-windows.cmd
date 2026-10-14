@echo off
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0Install.ps1" -Uninstall %*
set "NAAB_INSTALL_EXIT=%ERRORLEVEL%"
echo.
pause
exit /b %NAAB_INSTALL_EXIT%
