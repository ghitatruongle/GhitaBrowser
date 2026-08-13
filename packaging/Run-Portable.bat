@echo off
setlocal
set "GHITA_EXE=%~dp0GhitaBrowser.exe"
if not exist "%GHITA_EXE%" (
  echo ERROR: GhitaBrowser.exe was not found next to this launcher.
  exit /b 1
)
start "" "%GHITA_EXE%"
