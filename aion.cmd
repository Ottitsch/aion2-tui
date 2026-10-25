@echo off
rem Opens AION in Windows Terminal (the cube radar and the charts need true colour and Braille), or in this
rem console when Windows Terminal is not installed. Options are passed on, for example: aion.cmd --module cube
set "EXE=%~dp0target\release\aion.exe"
if not exist "%EXE%" (
  echo %EXE% is missing. Build it first: cargo build --release
  pause
  exit /b 1
)
rem As administrator (AION.lnk asks for that): run right in this window, which Windows opened as administrator.
rem A start wt.exe from here hands AION to a Windows Terminal that already runs without those rights, and
rem Windows then keeps ^ from it while a game that runs as administrator is in front.
fltmc >nul 2>&1
if %errorlevel%==0 (
  title AION
  "%EXE%" %*
  if errorlevel 1 pause
  exit /b
)
where wt.exe >nul 2>nul
if %errorlevel%==0 (
  start "" wt.exe --size 170,48 --title AION --suppressApplicationTitle -d "%~dp0." "%EXE%" %*
) else (
  "%EXE%" %*
  if errorlevel 1 pause
)
